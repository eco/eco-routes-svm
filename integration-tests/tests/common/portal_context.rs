use std::iter;

use anchor_lang::prelude::AccountMeta;
use anchor_lang::{system_program, InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::{event_authority_pda, Bytes32};
use portal::types::{Intent, Route};
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
    pub fn publish_intent(&mut self, intent: &Intent, route_hash: Bytes32) -> TransactionResult {
        let args = portal::instructions::PublishArgs {
            intent: intent.clone(),
            route_hash,
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
        intent: &Intent,
        vault: Pubkey,
        route_hash: Bytes32,
        allow_partial: bool,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::FundArgs {
            destination_chain: intent.destination_chain,
            route_hash,
            reward: intent.reward.clone(),
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
        intent: &Intent,
        vault: Pubkey,
        route_hash: Bytes32,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        creator: Pubkey,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::RefundArgs {
            destination_chain: intent.destination_chain,
            route_hash,
            reward: intent.reward.clone(),
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
        intent: &Intent,
        vault: Pubkey,
        route_hash: Bytes32,
        claimant: Pubkey,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        proof_closer: Pubkey,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
        remaining_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::WithdrawArgs {
            destination_chain: intent.destination_chain,
            route_hash,
            reward: intent.reward.clone(),
        };
        let instruction = portal::instruction::Withdraw { args };
        let accounts: Vec<_> = portal::accounts::Withdraw {
            payer: self.payer.pubkey(),
            claimant,
            vault,
            proof,
            proof_closer,
            prover: intent.reward.prover,
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
        route: &Route,
        reward_hash: Bytes32,
        claimant: Bytes32,
        executor: Pubkey,
        fulfill_marker: Pubkey,
        token_accounts: impl IntoIterator<Item = AccountMeta>,
        call_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        self.fulfill_intent_with_signers(
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
        intent_hash: Bytes32,
        source_chain: u64,
        fulfill_marker: Pubkey,
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
            intent_hash,
            hyper_prover::ID,
            source_chain,
            fulfill_marker,
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
        intent_hash: Bytes32,
        source_chain: u64,
        fulfill_marker: Pubkey,
        dispatcher: Pubkey,
        proof: Pubkey,
    ) -> TransactionResult {
        self.prove_intent(
            intent_hash,
            local_prover::ID,
            source_chain,
            fulfill_marker,
            dispatcher,
            vec![],
            vec![],
            vec![
                AccountMeta::new(proof, false),
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(system_program::ID, false),
                AccountMeta::new_readonly(event_authority_pda(&local_prover::ID).0, false),
                AccountMeta::new_readonly(local_prover::ID, false),
            ],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prove_intent(
        &mut self,
        intent_hash: Bytes32,
        prover: Pubkey,
        source_chain: u64,
        fulfill_marker: Pubkey,
        dispatcher: Pubkey,
        data: Vec<u8>,
        remaing_key_pairs: Vec<Keypair>,
        remaining_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::ProveArgs {
            prover,
            source_chain,
            intent_hash,
            data,
        };

        let instruction = portal::instruction::Prove { args };
        let accounts: Vec<_> = portal::accounts::Prove {
            prover,
            fulfill_marker,
            dispatcher,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(remaining_accounts)
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let key_pairs = iter::once(&self.payer)
            .chain(remaing_key_pairs.iter())
            .collect::<Vec<_>>();
        let transaction = Transaction::new(
            &key_pairs,
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}
