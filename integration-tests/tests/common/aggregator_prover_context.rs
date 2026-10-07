use aggregator_prover::instructions::MemberQuery;
use aggregator_prover::state::Config;
use anchor_lang::prelude::borsh;
use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::prover::{GetProofArgs, Proof};
use eco_svm_std::Bytes32;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{cleanup_recipient, Context, ProverQuery, TransactionResult};

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

    pub fn get_proof(&mut self, args: GetProofArgs, provers: &[Pubkey]) -> TransactionResult {
        let query = query_accounts(&args.intent_hash, provers);
        self.send_instruction(Instruction {
            program_id: aggregator_prover::ID,
            accounts: std::iter::once(AccountMeta::new_readonly(Config::pda().0, false))
                .chain(query.accounts)
                .collect(),
            data: aggregator_prover::instruction::GetProof {
                args: GetProofArgs {
                    data: query.data,
                    ..args
                },
            }
            .data(),
        })
    }

    pub fn cleanup_accounts(&self, intent_hash: &Bytes32, provers: &[Pubkey]) -> ProverQuery {
        let query = member_queries(provers.iter().map(|prover| {
            let cleanup = [
                AccountMeta::new(Proof::pda(intent_hash, prover).0, false),
                cleanup_recipient(prover, self.payer.pubkey()),
            ]
            .into();

            (*prover, cleanup)
        }));

        ProverQuery {
            accounts: std::iter::once(AccountMeta::new_readonly(Config::pda().0, false))
                .chain(query.accounts)
                .collect(),
            data: query.data,
        }
    }
}

pub fn query_accounts(intent_hash: &Bytes32, provers: &[Pubkey]) -> ProverQuery {
    member_queries(
        provers
            .iter()
            .map(|prover| (*prover, proof_query(intent_hash, prover))),
    )
}

fn proof_query(intent_hash: &Bytes32, prover: &Pubkey) -> ProverQuery {
    [AccountMeta::new_readonly(
        Proof::pda(intent_hash, prover).0,
        false,
    )]
    .into()
}

fn member_queries(members: impl IntoIterator<Item = (Pubkey, ProverQuery)>) -> ProverQuery {
    let (queries, accounts): (Vec<_>, Vec<_>) = members
        .into_iter()
        .map(|(prover, query)| {
            let ProverQuery { accounts, data } = query;
            let query = MemberQuery {
                account_count: accounts.len().try_into().unwrap(),
                data,
            };
            let accounts =
                std::iter::once(AccountMeta::new_readonly(prover, false)).chain(accounts);

            (query, accounts)
        })
        .unzip();

    ProverQuery {
        accounts: accounts.into_iter().flatten().collect(),
        data: borsh::to_vec(&queries).unwrap(),
    }
}
