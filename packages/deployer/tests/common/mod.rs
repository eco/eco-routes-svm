use std::ops::{Deref, DerefMut};

use anchor_lang::{system_program, InstructionData, ToAccountMetas};
use deployer::plan::Cluster;
use layerzero_prover::layerzero::{endpoint_settings_pda, DEVNET_SOLANA_EID, ENDPOINT_ID};
use litesvm::LiteSVM;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

pub mod litesvm_chain;

const PAYER_LAMPORTS: u64 = 10_000_000_000;
const AGGREGATOR_PROVER_BIN: &[u8] =
    include_bytes!("../../../../target/deploy/aggregator_prover.so");
const HYPER_PROVER_BIN: &[u8] = include_bytes!("../../../../target/deploy/hyper_prover.so");
const LAYERZERO_PROVER_BIN: &[u8] = include_bytes!("../../../../target/deploy/layerzero_prover.so");
const LOCAL_PROVER_BIN: &[u8] = include_bytes!("../../../../target/deploy/local_prover.so");
const POLYMER_PROVER_BIN: &[u8] = include_bytes!("../../../../target/deploy/polymer_prover.so");
const MOCK_LAYERZERO_ENDPOINT_BIN: &[u8] =
    include_bytes!("../../../../target/deploy/mock_layerzero_endpoint.so");

pub const RELEASE_PROGRAMS: [Pubkey; 5] = [
    aggregator_prover::ID,
    hyper_prover::ID,
    layerzero_prover::ID,
    local_prover::ID,
    polymer_prover::ID,
];

/// The release programs at their compiled IDs and the mock LayerZero endpoint, initialized for
/// devnet.
pub struct Context {
    svm: LiteSVM,
    payer: Keypair,
    /// What the chain reports as its genesis hash; devnet's unless a test changes it.
    pub genesis_hash: Hash,
}

impl Default for Context {
    fn default() -> Self {
        let mut svm = LiteSVM::new();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), PAYER_LAMPORTS).unwrap();
        [
            (aggregator_prover::ID, AGGREGATOR_PROVER_BIN),
            (hyper_prover::ID, HYPER_PROVER_BIN),
            (layerzero_prover::ID, LAYERZERO_PROVER_BIN),
            (local_prover::ID, LOCAL_PROVER_BIN),
            (polymer_prover::ID, POLYMER_PROVER_BIN),
            (ENDPOINT_ID, MOCK_LAYERZERO_ENDPOINT_BIN),
        ]
        .into_iter()
        .for_each(|(program, binary)| svm.add_program(program, binary).unwrap());
        let mut context = Self {
            svm,
            payer,
            genesis_hash: Cluster::Devnet.genesis_hash(),
        };
        context.init_endpoint();

        context
    }
}

impl Deref for Context {
    type Target = LiteSVM;

    fn deref(&self) -> &LiteSVM {
        &self.svm
    }
}

impl DerefMut for Context {
    fn deref_mut(&mut self) -> &mut LiteSVM {
        &mut self.svm
    }
}

impl Context {
    /// `None` makes the program immutable.
    pub fn set_upgrade_authority(&mut self, program: &Pubkey, authority: Option<Pubkey>) {
        let loader = self.get_account(program).unwrap().owner;
        let program_data_address = Pubkey::find_program_address(&[program.as_ref()], &loader).0;
        let mut program_data = self.get_account(&program_data_address).unwrap();
        let metadata = bincode::serialize(&UpgradeableLoaderState::ProgramData {
            slot: 0,
            upgrade_authority_address: authority,
        })
        .unwrap();
        program_data.data[..metadata.len()].copy_from_slice(&metadata);
        self.set_account(program_data_address, program_data)
            .unwrap();
    }

    fn init_endpoint(&mut self) {
        let payer = self.payer.pubkey();
        let instruction = Instruction {
            program_id: ENDPOINT_ID,
            accounts: mock_layerzero_endpoint::accounts::MockInit {
                payer,
                endpoint: endpoint_settings_pda().0,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mock_layerzero_endpoint::instruction::MockInit {
                eid: DEVNET_SOLANA_EID,
            }
            .data(),
        };
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&payer)),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction).unwrap();
    }
}
