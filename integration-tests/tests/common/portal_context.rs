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
    pub fn portal(&mut self) -> Portal<'_> {
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
        let payer = self.payer.insecure_clone();

        self.fund_intent_sponsored(
            &payer,
            &payer,
            true,
            destination,
            reward,
            vault,
            route_hash,
            allow_partial,
            token_transfer_accounts,
        )
    }

    /// Funds with a sponsor `payer` distinct from both `funder` and the
    /// transaction fee payer — the sponsored-relayer configuration the default
    /// `fund_intent` builder cannot express, since it pins payer to the fee
    /// payer. `payer`'s writability comes from `to_account_metas`, i.e. from the
    /// `Fund` struct's constraints, so it models an IDL-driven client rather
    /// than a hand-built meta.
    #[allow(clippy::too_many_arguments)]
    pub fn fund_intent_sponsored(
        &mut self,
        payer: &Keypair,
        fee_payer: &Keypair,
        payer_writable: bool,
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
            payer: payer.pubkey(),
            funder: self.funder.pubkey(),
            vault,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .map(
            |meta| match meta.pubkey == payer.pubkey() && !payer_writable {
                // hand-built rather than derived: every other fund test takes the
                // payer's writability from the same `Fund` struct it exercises, which
                // is what made the missing `mut` invisible in the first place
                true => AccountMeta::new_readonly(meta.pubkey, meta.is_signer),
                false => meta,
            },
        )
        .chain(token_transfer_accounts)
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let transaction = Transaction::new(
            &[fee_payer, payer, &self.funder],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&fee_payer.pubkey()),
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
        self.withdraw_intent_with_signers(
            destination,
            reward,
            vault,
            route_hash,
            claimant,
            proof,
            withdrawn_marker,
            proof_closer,
            token_transfer_accounts,
            remaining_accounts,
            vec![],
        )
    }

    /// `signers` are appended to the transaction and marked as signers in the
    /// account list — used for the claimant-signed destination override, the
    /// recovery route when the derived claimant ATA cannot receive.
    #[allow(clippy::too_many_arguments)]
    pub fn withdraw_intent_with_signers(
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
        signers: Vec<&Keypair>,
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
        .map(
            |meta| match signers.iter().any(|s| s.pubkey() == meta.pubkey) {
                true => AccountMeta {
                    is_signer: true,
                    ..meta
                },
                false => meta,
            },
        )
        .collect();
        let instruction = Instruction {
            program_id: portal::ID,
            accounts,
            data: instruction.data(),
        };

        let all_signers: Vec<&Keypair> = std::iter::once(&self.payer).chain(signers).collect();
        let transaction = Transaction::new(
            &all_signers,
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

    pub fn close_fulfill_marker(
        &mut self,
        intent_hash: Bytes32,
        fulfill_marker: Pubkey,
    ) -> TransactionResult {
        let payer = self.payer.pubkey();

        self.close_fulfill_markers(vec![(intent_hash, fulfill_marker)], payer, vec![])
    }

    /// `payer` is the marker's stored payer (the close authority and refund
    /// target), which is not necessarily the transaction's fee payer — the
    /// latter is always `self.payer`.
    pub fn close_fulfill_markers(
        &mut self,
        markers: Vec<(Bytes32, Pubkey)>,
        payer: Pubkey,
        additional_signers: Vec<&Keypair>,
    ) -> TransactionResult {
        let instructions: Vec<_> = markers
            .into_iter()
            .map(|(intent_hash, fulfill_marker)| Instruction {
                program_id: portal::ID,
                accounts: portal::accounts::CloseFulfillMarker {
                    payer,
                    fulfill_marker,
                }
                .to_account_metas(None),
                data: portal::instruction::CloseFulfillMarker {
                    args: portal::instructions::CloseFulfillMarkerArgs { intent_hash },
                }
                .data(),
            })
            .collect();

        let signers: Vec<_> = iter::once(&self.payer).chain(additional_signers).collect();
        let transaction = Transaction::new(
            &signers,
            Message::new(&instructions, Some(&self.payer.pubkey())),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prove_intent_via_hyper_prover(
        &mut self,
        intent_hashes: Vec<Bytes32>,
        source_chain_domain_id: u64,
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
            source_chain_domain_id,
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

    /// Like `prove_intent_via_hyper_prover` but accepts an external
    /// `unique_message` keypair so the caller can derive the
    /// `dispatched_message_pda` for subsequent instructions.
    #[allow(clippy::too_many_arguments)]
    pub fn prove_intent_with_unique_message(
        &mut self,
        intent_hashes: Vec<Bytes32>,
        source_chain_domain_id: u64,
        fulfill_markers: Vec<Pubkey>,
        dispatcher: Pubkey,
        prover_dispatcher: Pubkey,
        mailbox_program: Pubkey,
        data: Vec<u8>,
        unique_message: &Keypair,
        outbox_pda: Pubkey,
        dispatched_message_pda: Pubkey,
    ) -> TransactionResult {
        self.prove_intent(
            intent_hashes,
            hyper_prover::ID,
            source_chain_domain_id,
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
        source_chain_domain_id: u64,
        fulfill_markers: Vec<Pubkey>,
        dispatcher: Pubkey,
        proofs: Vec<Pubkey>,
    ) -> TransactionResult {
        self.prove_intent(
            intent_hashes,
            local_prover::ID,
            source_chain_domain_id,
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
        source_chain_domain_id: u64,
        fulfill_markers: Vec<Pubkey>,
        dispatcher: Pubkey,
        data: Vec<u8>,
        remaining_key_pairs: Vec<Keypair>,
        remaining_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = portal::instructions::ProveArgs {
            prover,
            source_chain_domain_id,
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
