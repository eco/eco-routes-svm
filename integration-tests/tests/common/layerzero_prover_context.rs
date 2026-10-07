use std::iter;

use anchor_lang::{system_program, AccountSerialize, InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::Bytes32;
use layerzero_prover::layerzero::{self, DEVNET_SOLANA_EID};
use layerzero_prover::state::{pda_payer_pda, Peer};
use portal::types::Reward;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{sol_amount, Context, TransactionResult};

const LAYERZERO_PROVER_BIN: &[u8] = include_bytes!("../../../target/deploy/layerzero_prover.so");

pub const BASE_EID: u32 = 30184;
pub const BASE_CHAIN_ID: u64 = 8453;
pub const OP_EID: u32 = 30111;
pub const OP_CHAIN_ID: u64 = 10;
pub const COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
/// Receives the mock endpoint's flat fee in `send_message` tests.
pub const TREASURY: Pubkey = Pubkey::new_from_array([0x7e; 32]);

/// An EVM peer whose address is a left-padded 20-byte address of `byte`s.
pub fn evm_peer(eid: u32, chain_id: u64, byte: u8) -> Peer {
    let mut address = [0u8; 32];
    address[12..].fill(byte);

    Peer {
        eid,
        address: address.into(),
        chain_id,
    }
}

pub fn peers() -> Vec<Peer> {
    vec![
        evm_peer(BASE_EID, BASE_CHAIN_ID, 0xba),
        evm_peer(OP_EID, OP_CHAIN_ID, 0x0b),
    ]
}

#[derive(Deref, DerefMut)]
pub struct LayerZeroProver<'a>(&'a mut Context);

impl Context {
    pub fn layerzero_prover(&mut self) -> LayerZeroProver<'_> {
        LayerZeroProver(self)
    }
}

impl LayerZeroProver<'_> {
    /// Adds the program with `authority` as upgrade authority, initializes the
    /// mock endpoint's settings account and funds `pda_payer`.
    pub fn install(&mut self, authority: Pubkey) {
        self.add_program(layerzero_prover::ID, LAYERZERO_PROVER_BIN)
            .unwrap();
        self.set_upgrade_authority(Some(authority));

        let payer = self.payer.pubkey();
        let instruction = Instruction {
            program_id: layerzero::ENDPOINT_ID,
            accounts: mock_layerzero_endpoint::accounts::MockInit {
                payer,
                endpoint: layerzero::endpoint_settings_pda().0,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mock_layerzero_endpoint::instruction::MockInit {
                eid: DEVNET_SOLANA_EID,
            }
            .data(),
        };
        self.send(vec![instruction], &[]).unwrap();

        self.airdrop(&pda_payer_pda().0, sol_amount(10.0)).unwrap();
    }

    pub fn program_data(&self) -> Pubkey {
        let program = self.get_account(&layerzero_prover::ID).unwrap();
        Pubkey::find_program_address(&[layerzero_prover::ID.as_ref()], &program.owner).0
    }

    pub fn set_upgrade_authority(&mut self, authority: Option<Pubkey>) {
        let address = self.program_data();
        let mut program_data = self.get_account(&address).unwrap();
        let metadata = bincode::serialize(&UpgradeableLoaderState::ProgramData {
            slot: 0,
            upgrade_authority_address: authority,
        })
        .unwrap();
        program_data.data[..metadata.len()].copy_from_slice(&metadata);
        self.set_account(address, program_data).unwrap();
    }

    /// Simulates `solana program set-upgrade-authority --final`.
    pub fn finalize(&mut self) {
        self.set_upgrade_authority(None);
    }

    /// Sends `instructions` after a 1.4M CU limit, paid by the context payer
    /// plus any extra `signers`.
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

    /// A funded, native-only intent on this (source) chain naming `prover`.
    pub fn funded_native_intent(
        &mut self,
        destination: u64,
        prover: Pubkey,
    ) -> (Reward, Bytes32, Bytes32) {
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

/// Serializes an Anchor account (mock or ours) for `set_account`.
pub fn anchor_account_data<T: AccountSerialize>(account: &T) -> Vec<u8> {
    let mut data = Vec::new();
    account.try_serialize(&mut data).unwrap();
    data
}
