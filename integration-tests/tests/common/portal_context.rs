use std::iter;

use anchor_lang::prelude::AccountMeta;
use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::{event_authority_pda, Bytes32};
use portal::types::{Reward, Route};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{hyperlane_context, Context, TransactionResult, COMPUTE_UNIT_LIMIT};

#[derive(Deref, DerefMut)]
pub struct Portal<'a>(&'a mut Context);

impl Context {
    pub fn portal(&mut self) -> Portal {
        Portal(self)
    }
}

impl Portal<'_> {
    pub fn publish_intent(
        &mut self,
        destination: u64,
        route: Vec<u8>,
        reward: Reward,
    ) -> TransactionResult {
        let args = portal::instructions::PublishArgs {
            destination,
            route,
            reward,
        };
        let instruction = portal::instruction::Publish { args };
        let accounts: Vec<_> = portal::accounts::Publish {}.to_account_metas(None);
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn fund_intent(
        &mut self,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        allow_partial: bool,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::FundArgs {
            destination,
            route_hash,
            reward,
            allow_partial,
        };
        let instruction = portal::instruction::Fund { args };
        let accounts: Vec<_> = portal::accounts::Fund {
            payer: self.payer.pubkey(),
            funder: self.funder.pubkey(),
            vault,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_transfer_accounts)
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer, &self.funder],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&self.payer.pubkey()),
            ),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn refund_intent(
        &mut self,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        creator: Pubkey,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::RefundArgs {
            destination,
            route_hash,
            reward,
        };
        let instruction = portal::instruction::Refund { args };
        let accounts: Vec<_> = portal::accounts::Refund {
            payer: self.payer.pubkey(),
            creator,
            vault,
            proof,
            withdrawn_marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_transfer_accounts)
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&self.payer.pubkey()),
            ),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn withdraw_intent(
        &mut self,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        claimant: Pubkey,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        proof_closer: Pubkey,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
        remaining_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let prover = reward.prover;
        let args = portal::instructions::WithdrawArgs {
            destination,
            route_hash,
            reward,
        };
        let instruction = portal::instruction::Withdraw { args };
        let accounts: Vec<_> = portal::accounts::Withdraw {
            payer: self.payer.pubkey(),
            claimant,
            vault,
            proof,
            proof_closer,
            prover,
            withdrawn_marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_transfer_accounts)
        .chain(remaining_accounts)
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&self.payer.pubkey()),
            ),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fulfill_intent(
        &mut self,
        intent_hash: Bytes32,
        route: &Route,
        reward_hash: Bytes32,
        claimant: Bytes32,
        executor: Pubkey,
        fulfill_marker: Pubkey,
        token_accounts: impl IntoIterator<Item = AccountMeta>,
        call_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        self.fulfill_intent_with_signers(
            intent_hash,
            route,
            reward_hash,
            claimant,
            executor,
            fulfill_marker,
            token_accounts,
            call_accounts,
            vec![],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fulfill_intent_with_signers(
        &mut self,
        intent_hash: Bytes32,
        route: &Route,
        reward_hash: Bytes32,
        claimant: Bytes32,
        executor: Pubkey,
        fulfill_marker: Pubkey,
        token_accounts: impl IntoIterator<Item = AccountMeta>,
        call_accounts: impl IntoIterator<Item = AccountMeta>,
        additional_signers: Vec<&Keypair>,
    ) -> TransactionResult {
        let args = portal::instructions::FulfillArgs {
            intent_hash,
            route: route.clone(),
            reward_hash,
            claimant,
        };
        let instruction = portal::instruction::Fulfill { args };
        let accounts: Vec<_> = portal::accounts::Fulfill {
            payer: self.payer.pubkey(),
            solver: self.solver.pubkey(),
            executor,
            fulfill_marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_accounts)
        .chain(call_accounts)
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let signers: Vec<_> = vec![&self.payer, &self.solver]
            .into_iter()
            .chain(additional_signers)
            .collect();

        let transaction = Transaction::new(
            &signers,
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&self.payer.pubkey()),
            ),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prove_intent_via_hyper_prover(
        &mut self,
        intent_hashes: Vec<Bytes32>,
        source: u64,
        fulfill_markers: Vec<Pubkey>,
        dispatcher: Pubkey,
        prover_dispatcher: Pubkey,
        mailbox_program: Pubkey,
        data: Vec<u8>,
    ) -> TransactionResult {
        let outbox_pda = hyperlane_context::outbox_pda();
        let unique_message = Keypair::new();
        let dispatched_message_pda =
            hyperlane_context::dispatched_message_pda(&unique_message.pubkey());

        self.prove_intent(
            intent_hashes,
            hyper_prover::ID,
            source,
            fulfill_markers,
            dispatcher,
            data,
            vec![unique_message.insecure_clone()],
            vec![
                AccountMeta::new_readonly(prover_dispatcher, false),
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(outbox_pda, false),
                AccountMeta::new_readonly(spl_noop::ID, false),
                AccountMeta::new_readonly(unique_message.pubkey(), true),
                AccountMeta::new(dispatched_message_pda, false),
                AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
                AccountMeta::new_readonly(mailbox_program, false),
            ],
        )
    }

    pub fn prove_intent_via_local_prover(
        &mut self,
        intent_hashes: Vec<Bytes32>,
        source: u64,
        fulfill_markers: Vec<Pubkey>,
        dispatcher: Pubkey,
        proofs: Vec<Pubkey>,
    ) -> TransactionResult {
        self.prove_intent(
            intent_hashes,
            local_prover::ID,
            source,
            fulfill_markers,
            dispatcher,
            vec![],
            vec![],
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
                AccountMeta::new_readonly(event_authority_pda(&local_prover::ID).0, false),
                AccountMeta::new_readonly(local_prover::ID, false),
            ]
            .into_iter()
            .chain(
                proofs
                    .into_iter()
                    .map(|proof| AccountMeta::new(proof, false)),
            ),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prove_intent(
        &mut self,
        intent_hashes: Vec<Bytes32>,
        prover: Pubkey,
        source: u64,
        fulfill_markers: Vec<Pubkey>,
        dispatcher: Pubkey,
        data: Vec<u8>,
        remaining_key_pairs: Vec<Keypair>,
        remaining_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::ProveArgs {
            prover,
            source,
            intent_hashes,
            data,
        };

        let instruction = portal::instruction::Prove { args };
        let accounts: Vec<_> = portal::accounts::Prove { prover, dispatcher }
            .to_account_metas(None)
            .into_iter()
            .chain(
                fulfill_markers
                    .into_iter()
                    .map(|fulfill_marker| AccountMeta {
                        pubkey: fulfill_marker,
                        is_signer: false,
                        is_writable: false,
                    }),
            )
            .chain(remaining_accounts)
            .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let key_pairs = iter::once(&self.payer)
            .chain(remaining_key_pairs.iter())
            .collect::<Vec<_>>();
        let transaction = Transaction::new(
            &key_pairs,
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}
