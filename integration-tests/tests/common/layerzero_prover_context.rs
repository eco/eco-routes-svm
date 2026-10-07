use std::iter;

use anchor_lang::{system_program, AccountSerialize, InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::Bytes32;
use layerzero_prover::instructions::{InitArgs, PathConfig, ADDRESS_LOOKUP_TABLE_PROGRAM_ID};
use layerzero_prover::layerzero::{
    self, ExecutorConfig, UlnConfig, DEVNET_SOLANA_EID, ENDPOINT_ID, ULN_ID,
};
use layerzero_prover::state::{pda_payer_pda, LzReceiveTypesAccount, Peer, Store};
use portal::types::Reward;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::account::Account;
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

/// A fully pinned path config: two required DVNs (sorted, as ULN302 requires),
/// explicit confirmations and executor.
pub fn path_config() -> PathConfig {
    let mut dvns = vec![
        Pubkey::new_from_array([1; 32]),
        Pubkey::new_from_array([2; 32]),
    ];
    dvns.sort();
    let uln = UlnConfig {
        confirmations: 15,
        required_dvn_count: 2,
        optional_dvn_count: 0,
        optional_dvn_threshold: 0,
        required_dvns: dvns,
        optional_dvns: vec![],
    };

    PathConfig {
        send_uln: uln.clone(),
        receive_uln: uln,
        executor: ExecutorConfig {
            max_message_size: 10_000,
            executor: Pubkey::new_from_array([3; 32]),
        },
    }
}

impl LayerZeroProver<'_> {
    pub fn init(&mut self, authority: &Keypair, peers: Vec<Peer>) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::Init {
                payer: self.payer.pubkey(),
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                lz_receive_types: LzReceiveTypesAccount::pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                endpoint_event_authority: layerzero::endpoint_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::Init {
                args: InitArgs { peers },
            }
            .data(),
        };

        self.send(vec![instruction], &[authority])
    }

    pub fn init_path(&mut self, authority: &Keypair, peer: &Peer) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::InitPath {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                pda_payer: pda_payer_pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                nonce: layerzero::nonce_pda(&store, peer.eid, &peer.address).0,
                pending_nonce: layerzero::pending_nonce_pda(&store, peer.eid, &peer.address).0,
                send_library_config: layerzero::send_library_config_pda(&store, peer.eid).0,
                receive_library_config: layerzero::receive_library_config_pda(&store, peer.eid).0,
                message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0)
                    .0,
                endpoint_event_authority: layerzero::endpoint_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::InitPath { eid: peer.eid }.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    pub fn set_path_config(
        &mut self,
        authority: &Keypair,
        eid: u32,
        config: PathConfig,
    ) -> TransactionResult {
        let store = Store::pda().0;
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::SetPathConfig {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store,
                pda_payer: pda_payer_pda().0,
                system_program: system_program::ID,
                endpoint_program: ENDPOINT_ID,
                oapp_registry: layerzero::oapp_registry_pda(&store).0,
                message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0)
                    .0,
                uln_settings: layerzero::uln_settings_pda().0,
                uln_program: ULN_ID,
                uln_send_config: layerzero::uln_send_config_pda(eid, &store).0,
                uln_receive_config: layerzero::uln_receive_config_pda(eid, &store).0,
                uln_default_send_config: layerzero::uln_default_send_config_pda(eid).0,
                uln_default_receive_config: layerzero::uln_default_receive_config_pda(eid).0,
                uln_event_authority: layerzero::uln_event_authority().0,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::SetPathConfig { eid, config }.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    /// Stages an account owned by the lookup-table program (contents are not
    /// read on-chain; the executor reads the table off-chain).
    pub fn create_alt(&mut self) -> Pubkey {
        let alt = Pubkey::new_unique();
        self.set_account(
            alt,
            Account {
                lamports: 1_000_000_000,
                data: vec![0; 56],
                owner: ADDRESS_LOOKUP_TABLE_PROGRAM_ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
        alt
    }

    pub fn set_alt(&mut self, authority: &Keypair, alt: Pubkey) -> TransactionResult {
        let instruction = Instruction {
            program_id: layerzero_prover::ID,
            accounts: layerzero_prover::accounts::SetAlt {
                authority: authority.pubkey(),
                program: layerzero_prover::ID,
                program_data: self.program_data(),
                store: Store::pda().0,
                alt,
            }
            .to_account_metas(None),
            data: layerzero_prover::instruction::SetAlt {}.data(),
        };

        self.send(vec![instruction], &[authority])
    }

    /// install + init + every peer's path and config + ALT.
    pub fn setup(&mut self) -> Keypair {
        let authority = Keypair::new();
        self.install(authority.pubkey());
        self.init(&authority, peers()).unwrap();
        peers().iter().for_each(|peer| {
            self.init_path(&authority, peer).unwrap();
            self.set_path_config(&authority, peer.eid, path_config())
                .unwrap();
        });
        let alt = self.create_alt();
        self.set_alt(&authority, alt).unwrap();

        authority
    }
}
