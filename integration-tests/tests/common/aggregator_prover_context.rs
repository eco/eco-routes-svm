use aggregator_prover::state::Config;
use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::prover::{Proof, ValidateProofArgs};
use eco_svm_std::Bytes32;
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

    pub fn init(&mut self, authority: &Keypair, provers: Vec<Pubkey>) -> TransactionResult {
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
            provers
                .iter()
                .map(|prover| AccountMeta::new_readonly(*prover, false)),
        )
        .collect();
        let instruction = Instruction {
            program_id: aggregator_prover::ID,
            accounts,
            data: aggregator_prover::instruction::Init {}.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, authority],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn validate_proof(
        &mut self,
        args: ValidateProofArgs,
        provers: &[Pubkey],
    ) -> TransactionResult {
        let accounts = std::iter::once(AccountMeta::new_readonly(Config::pda().0, false))
            .chain(provers.iter().flat_map(|prover| {
                [
                    AccountMeta::new_readonly(*prover, false),
                    AccountMeta::new_readonly(Proof::pda(&args.intent_hash, prover).0, false),
                ]
            }))
            .collect();
        self.send_instruction(Instruction {
            program_id: aggregator_prover::ID,
            accounts,
            data: aggregator_prover::instruction::ValidateProof { args }.data(),
        })
    }

    pub fn cleanup_accounts(&self, intent_hash: &Bytes32, prover: Pubkey) -> Vec<AccountMeta> {
        let recipient = if prover == hyper_prover::ID {
            hyper_prover::state::pda_payer_pda().0
        } else {
            self.payer.pubkey()
        };
        vec![
            AccountMeta::new_readonly(Config::pda().0, false),
            AccountMeta::new_readonly(prover, false),
            AccountMeta::new(Proof::pda(intent_hash, &prover).0, false),
            AccountMeta::new(recipient, prover != hyper_prover::ID),
        ]
    }
}
