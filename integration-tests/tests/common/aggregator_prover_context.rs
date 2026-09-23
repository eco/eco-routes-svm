use aggregator_prover::instructions::AggregateArgs;
use aggregator_prover::state::Config;
use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::event_authority_pda;
use eco_svm_std::prover::Proof;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{Context, TransactionResult};

const AGGREGATOR_PROVER_BIN: &[u8] = include_bytes!("../../../target/deploy/aggregator_prover.so");

#[derive(Deref, DerefMut)]
pub struct AggregatorProver<'a>(&'a mut Context);

impl Context {
    pub fn aggregator_prover(&mut self) -> AggregatorProver<'_> {
        AggregatorProver(self)
    }
}

impl AggregatorProver<'_> {
    pub fn install(&mut self, authority: Pubkey) {
        self.add_program(aggregator_prover::ID, AGGREGATOR_PROVER_BIN)
            .unwrap();
        let program = self.get_account(&aggregator_prover::ID).unwrap();
        let program_data_address =
            Pubkey::find_program_address(&[aggregator_prover::ID.as_ref()], &program.owner).0;
        let mut program_data = self.get_account(&program_data_address).unwrap();
        let metadata = bincode::serialize(&UpgradeableLoaderState::ProgramData {
            slot: 0,
            upgrade_authority_address: Some(authority),
        })
        .unwrap();
        program_data.data[..metadata.len()].copy_from_slice(&metadata);
        self.set_account(program_data_address, program_data)
            .unwrap();
    }

    pub fn init(&mut self, authority: &Keypair, members: Vec<Pubkey>) -> TransactionResult {
        let program = self.get_account(&aggregator_prover::ID).unwrap();
        let program_data =
            Pubkey::find_program_address(&[aggregator_prover::ID.as_ref()], &program.owner).0;
        let accounts = aggregator_prover::accounts::Init {
            payer: self.payer.pubkey(),
            authority: authority.pubkey(),
            program: aggregator_prover::ID,
            program_data,
            config: Config::pda().0,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(
            members
                .iter()
                .map(|member| AccountMeta::new_readonly(*member, false)),
        )
        .collect();
        let instruction = Instruction {
            program_id: aggregator_prover::ID,
            accounts,
            data: aggregator_prover::instruction::Init { members }.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, authority],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn build_aggregate_instruction(
        &self,
        args: AggregateArgs,
        members: &[Pubkey],
    ) -> Instruction {
        let accounts = aggregator_prover::accounts::Aggregate {
            payer: self.payer.pubkey(),
            config: Config::pda().0,
            system_program: anchor_lang::system_program::ID,
            event_authority: event_authority_pda(&aggregator_prover::ID).0,
            program: aggregator_prover::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(
            args.proof_data
                .intent_hashes_claimants
                .iter()
                .flat_map(|intent| {
                    std::iter::once(AccountMeta::new(
                        Proof::pda(&intent.intent_hash, &aggregator_prover::ID).0,
                        false,
                    ))
                    .chain(members.iter().map(|member| {
                        AccountMeta::new_readonly(Proof::pda(&intent.intent_hash, member).0, false)
                    }))
                }),
        )
        .collect();

        Instruction {
            program_id: aggregator_prover::ID,
            accounts,
            data: aggregator_prover::instruction::Aggregate { args }.data(),
        }
    }

    pub fn aggregate(&mut self, args: AggregateArgs, members: &[Pubkey]) -> TransactionResult {
        let instruction = self.build_aggregate_instruction(args, members);

        self.send_instruction(instruction)
    }

    pub fn send_instruction(&mut self, instruction: Instruction) -> TransactionResult {
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}
