use std::iter;

use anchor_lang::prelude::borsh;
use anchor_lang::{system_program, AnchorSerialize, InstructionData, ToAccountMetas};
use cctp_prover::cctp::{
    self, MESSAGE_TRANSMITTER_ACCOUNT_DISCRIMINATOR, MESSAGE_TRANSMITTER_V2_ID, SOLANA_DOMAIN,
};
use cctp_prover::instructions::InitArgs;
use cctp_prover::state::{pda_payer_pda, Config, Peer};
use derive_more::{Deref, DerefMut};
use eco_svm_std::prover::{Proof, ProofData};
use eco_svm_std::Bytes32;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::account::Account;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;
use tiny_keccak::{Hasher, Keccak};

use crate::common::{program_data_address, sol_amount, Context, TransactionResult};

pub const ETHEREUM_DOMAIN: u32 = 0;
pub const ETHEREUM_CHAIN_ID: u64 = 1;
pub const OP_DOMAIN: u32 = 2;
pub const BASE_DOMAIN: u32 = 6;
pub const BASE_CHAIN_ID: u64 = 8453;
pub const COMPUTE_UNIT_LIMIT: u32 = 400_000;

/// An EVM peer whose sender is a left-padded 20-byte address of `byte`s.
pub fn evm_peer(domain: u32, chain_id: u64, byte: u8) -> Peer {
    let mut sender = [0u8; 32];
    sender[12..].fill(byte);

    Peer {
        domain,
        chain_id,
        sender: sender.into(),
    }
}

/// Ethereum (domain 0, the case MessageBridgeProver's base rejects) and Base.
pub fn peers() -> Vec<Peer> {
    vec![
        evm_peer(ETHEREUM_DOMAIN, ETHEREUM_CHAIN_ID, 0xee),
        evm_peer(BASE_DOMAIN, BASE_CHAIN_ID, 0xba),
    ]
}

pub fn build_init_instruction(payer: &Pubkey, authority: &Pubkey, peers: Vec<Peer>) -> Instruction {
    Instruction {
        program_id: cctp_prover::ID,
        accounts: cctp_prover::accounts::Init {
            payer: *payer,
            authority: *authority,
            program: cctp_prover::ID,
            program_data: program_data_address(&cctp_prover::ID),
            config: Config::pda().0,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: cctp_prover::instruction::Init {
            args: InitArgs { peers },
        }
        .data(),
    }
}

#[derive(Deref, DerefMut)]
pub struct CctpProver<'a>(&'a mut Context);

impl Context {
    pub fn cctp_prover(&mut self) -> CctpProver<'_> {
        CctpProver(self)
    }
}

impl CctpProver<'_> {
    /// Makes `authority` the program's upgrade authority and funds `pda_payer`.
    pub fn install(&mut self, authority: Pubkey) {
        self.set_upgrade_authority(&cctp_prover::ID, Some(authority));
        self.airdrop(&pda_payer_pda().0, sol_amount(10.0)).unwrap();
        install_transmitter(self, &[TestAttester::new(TEST_ATTESTER_SECRET)], 1);
    }

    /// Simulates `solana program set-upgrade-authority --final`.
    pub fn finalize(&mut self) {
        self.set_upgrade_authority(&cctp_prover::ID, None);
    }

    pub fn init(&mut self, authority: &Keypair, peers: Vec<Peer>) -> TransactionResult {
        let instruction = build_init_instruction(&self.payer.pubkey(), &authority.pubkey(), peers);

        self.send(vec![instruction], &[authority])
    }

    /// Sends `instructions` after a `COMPUTE_UNIT_LIMIT`, paid by the context
    /// payer plus any extra `signers`.
    pub fn send(
        &mut self,
        instructions: Vec<Instruction>,
        signers: &[&Keypair],
    ) -> TransactionResult {
        let payer = self.payer.insecure_clone();
        let instructions: Vec<_> = iter::once(ComputeBudgetInstruction::set_compute_unit_limit(
            COMPUTE_UNIT_LIMIT,
        ))
        .chain(instructions)
        .collect();
        let signers: Vec<&Keypair> = iter::once(&payer).chain(signers.iter().copied()).collect();
        let transaction = Transaction::new(
            &signers,
            Message::new(&instructions, Some(&payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}

/// Synthetic attester key, generated for these tests only.
pub const TEST_ATTESTER_SECRET: [u8; 32] = [0x11; 32];

fn keccak(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut hasher = Keccak::v256();
    hasher.update(data);
    hasher.finalize(&mut out);
    out
}

/// An EVM-style attester: Circle stores its 20-byte address left-padded with
/// zeros as a `Pubkey` (`MessageTransmitter::recover_attester`).
pub struct TestAttester(libsecp256k1::SecretKey);

impl TestAttester {
    pub fn new(secret: [u8; 32]) -> Self {
        Self(libsecp256k1::SecretKey::parse(&secret).unwrap())
    }

    pub fn address(&self) -> Pubkey {
        let public = libsecp256k1::PublicKey::from_secret_key(&self.0).serialize();
        let mut address = keccak(&public[1..]);
        address[..12].fill(0);
        Pubkey::new_from_array(address)
    }

    /// `r || s || v` over `keccak256(message)`, `v = recovery id + 27`.
    /// libsecp256k1 normalizes `s` low, which Circle requires.
    pub fn sign(&self, message: &[u8]) -> [u8; 65] {
        let digest = libsecp256k1::Message::parse(&keccak(message));
        let (signature, recovery) = libsecp256k1::sign(&digest, &self.0);
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(&signature.serialize());
        out[64] = recovery.serialize() + 27;
        out
    }
}

/// Signatures in strictly increasing attester-address order, as Circle requires.
pub fn attest(attesters: &[TestAttester], message: &[u8]) -> Vec<u8> {
    let mut ordered: Vec<_> = attesters.iter().collect();
    ordered.sort_by_key(|attester| attester.address());
    ordered
        .iter()
        .flat_map(|attester| attester.sign(message))
        .collect()
}

/// Circle's `MessageTransmitter` account (state.rs), field for field.
#[derive(AnchorSerialize)]
struct MessageTransmitterState {
    owner: Pubkey,
    pending_owner: Pubkey,
    attester_manager: Pubkey,
    pauser: Pubkey,
    paused: bool,
    local_domain: u32,
    version: u32,
    signature_threshold: u32,
    enabled_attesters: Vec<Pubkey>,
    max_message_body_size: u64,
}

/// Plants the transmitter's state PDA as Solana's domain with `attesters`
/// enabled at `threshold`, the way `initialize` + `enable_attester` leave it.
pub fn install_transmitter(context: &mut Context, attesters: &[TestAttester], threshold: u32) {
    let state = MessageTransmitterState {
        owner: Pubkey::new_unique(),
        pending_owner: Pubkey::default(),
        attester_manager: Pubkey::new_unique(),
        pauser: Pubkey::new_unique(),
        paused: false,
        local_domain: SOLANA_DOMAIN,
        version: cctp::MESSAGE_VERSION,
        signature_threshold: threshold,
        enabled_attesters: attesters.iter().map(TestAttester::address).collect(),
        max_message_body_size: 8192,
    };
    let data: Vec<u8> = MESSAGE_TRANSMITTER_ACCOUNT_DISCRIMINATOR
        .into_iter()
        .chain(borsh::to_vec(&state).unwrap())
        .collect();
    let lamports = context.get_sysvar::<Rent>().minimum_balance(data.len());
    context
        .set_account(
            cctp::message_transmitter_pda().0,
            Account {
                lamports,
                data,
                owner: MESSAGE_TRANSMITTER_V2_ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
}

/// `prove`'s accounts after the Portal dispatcher (which Portal prepends).
pub fn prove_accounts(payer: &Pubkey, message_sent_event_data: &Pubkey) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(cctp_prover::state::sender_authority_pda().0, false),
        AccountMeta::new(*payer, true),
        AccountMeta::new(cctp::message_transmitter_pda().0, false),
        AccountMeta::new(*message_sent_event_data, true),
        AccountMeta::new_readonly(cctp_prover::ID, false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(MESSAGE_TRANSMITTER_V2_ID, false),
    ]
}

/// An EVM `CCTPProver` address, left-padded: `prove`'s `data`.
pub fn evm_recipient(byte: u8) -> Vec<u8> {
    let mut recipient = [0u8; 32];
    recipient[12..].fill(byte);
    recipient.to_vec()
}

pub struct Inbound {
    pub message: Vec<u8>,
    pub attestation: Vec<u8>,
}

pub fn evm_sender(byte: u8) -> [u8; 32] {
    let mut sender = [0u8; 32];
    sender[12..].fill(byte);
    sender
}

pub fn proof_pdas(proof_data: &ProofData) -> Vec<Pubkey> {
    proof_data
        .intent_hashes_claimants
        .iter()
        .map(|pair| Proof::pda(&pair.intent_hash, &cctp_prover::ID).0)
        .collect()
}

impl CctpProver<'_> {
    /// An attested EVM -> Solana message, as Iris returns it: random nonce,
    /// `finality` as executed, signed by the installed test attester.
    pub fn inbound(
        &self,
        source_domain: u32,
        sender: [u8; 32],
        finality: u32,
        proof_data: ProofData,
    ) -> Inbound {
        let message = cctp::format_message(
            source_domain,
            SOLANA_DOMAIN,
            rand::random(),
            &Pubkey::new_from_array(sender),
            &cctp_prover::ID,
            cctp::MIN_FINALITY_THRESHOLD,
            finality,
            &proof_data.to_bytes(),
        );
        let attestation = attest(&[TestAttester::new(TEST_ATTESTER_SECRET)], &message);

        Inbound {
            message,
            attestation,
        }
    }

    pub fn receive(&mut self, inbound: &Inbound, proofs: &[Pubkey]) -> TransactionResult {
        let payer = self.payer.pubkey();
        let ix = cctp::receive_message_instruction(
            &payer,
            inbound.message.clone(),
            inbound.attestation.clone(),
            proofs,
        )
        .unwrap();

        self.send(vec![ix], &[])
    }

    pub fn deliver(
        &mut self,
        source_domain: u32,
        sender: [u8; 32],
        finality: u32,
        proof_data: ProofData,
    ) -> TransactionResult {
        let proofs = proof_pdas(&proof_data);
        let inbound = self.inbound(source_domain, sender, finality, proof_data);

        self.receive(&inbound, &proofs)
    }

    /// A funded, native-only intent on this (source) chain naming `prover`.
    pub fn funded_native_intent(
        &mut self,
        destination: u64,
        prover: Pubkey,
    ) -> (portal::types::Reward, Bytes32, Bytes32) {
        let (_, _, mut reward) = self.rand_intent();
        reward.prover = prover;
        reward.tokens.clear();
        let route_hash: Bytes32 = rand::random::<[u8; 32]>().into();
        let hash = portal::types::intent_hash(destination, &route_hash, &reward.hash());
        let vault = portal::state::vault_pda(&hash).0;
        let funder = self.funder.pubkey();
        self.airdrop(&funder, reward.native_amount).unwrap();
        self.portal()
            .fund_intent(
                destination,
                reward.clone(),
                vault,
                route_hash,
                false,
                Vec::<AccountMeta>::new(),
            )
            .unwrap();

        (reward, route_hash, hash)
    }
}

pub fn cleanup_tail(intent_hash: &eco_svm_std::Bytes32) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(Proof::pda(intent_hash, &cctp_prover::ID).0, false),
        crate::common::cleanup_recipient(&cctp_prover::ID, Pubkey::default()),
    ]
}
