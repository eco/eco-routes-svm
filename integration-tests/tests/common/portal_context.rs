use std::iter;

use anchor_lang::prelude::AccountMeta;
use anchor_lang::{InstructionData, ToAccountMetas};
use derive_more::{Deref, DerefMut};
use eco_svm_std::{event_authority_pda, Bytes32};
use portal::instructions::RefundKind;
use portal::state::proof_closer_pda;
use portal::types::{Reward, Route};
use solana_compute_budget_interface::ComputeBudgetInstruction;
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
        kind: RefundKind,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        creator: Pubkey,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        self.refund_intent_with_accounts(
            destination,
            reward.clone(),
            vault,
            route_hash,
            proof,
            withdrawn_marker,
            creator,
            kind,
            Some(reward.prover),
            token_transfer_accounts,
            vec![],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn refund_cancelled_intent(
        &mut self,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        creator: Pubkey,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
        prover_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let prover = reward.prover;

        self.refund_intent_with_accounts(
            destination,
            reward,
            vault,
            route_hash,
            proof,
            withdrawn_marker,
            creator,
            RefundKind::Cancelled,
            Some(prover),
            token_transfer_accounts,
            prover_accounts,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn refund_intent_with_accounts(
        &mut self,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        creator: Pubkey,
        kind: RefundKind,
        prover: Option<Pubkey>,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
        prover_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let instruction = self.refund_intent_instruction(
            destination,
            reward,
            vault,
            route_hash,
            proof,
            withdrawn_marker,
            creator,
            kind,
            prover,
            token_transfer_accounts,
            prover_accounts,
        );

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

    /// A `None` optional account is passed as the portal program ID, Anchor's
    /// placeholder for an omitted account.
    #[allow(clippy::too_many_arguments)]
    pub fn refund_intent_instruction(
        &self,
        destination: u64,
        reward: Reward,
        vault: Pubkey,
        route_hash: Bytes32,
        proof: Pubkey,
        withdrawn_marker: Pubkey,
        creator: Pubkey,
        kind: RefundKind,
        prover: Option<Pubkey>,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
        prover_accounts: Vec<AccountMeta>,
    ) -> Instruction {
        let prover_accounts = if kind == RefundKind::Withdrawn
            || !self
                .get_account(&reward.prover)
                .is_some_and(|account| account.executable)
        {
            vec![]
        } else {
            std::iter::once(AccountMeta::new_readonly(proof, false))
                .chain(prover_accounts)
                .collect()
        };
        let args = portal::instructions::RefundArgs {
            destination,
            route_hash,
            reward,
            kind,
            prover_account_count: prover_accounts.len().try_into().unwrap(),
        };
        let accounts: Vec<_> = portal::accounts::Refund {
            payer: self.payer.pubkey(),
            creator,
            vault,
            prover,
            withdrawn_marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_transfer_accounts)
        .chain(prover_accounts)
        .collect();

        Instruction {
            program_id: portal::ID,
            accounts,
            data: portal::instruction::Refund { args }.data(),
        }
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
            prover,
            withdrawn_marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_transfer_accounts)
        .chain(std::iter::once(AccountMeta::new_readonly(proof, false)))
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
        let transaction = self.fulfill_intent_transaction(
            intent_hash,
            route,
            reward_hash,
            claimant,
            executor,
            fulfill_marker,
            token_accounts,
            call_accounts,
            additional_signers,
        );

        self.send_transaction(transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fulfill_intent_transaction(
        &self,
        intent_hash: Bytes32,
        route: &Route,
        reward_hash: Bytes32,
        claimant: Bytes32,
        executor: Pubkey,
        fulfill_marker: Pubkey,
        token_accounts: impl IntoIterator<Item = AccountMeta>,
        call_accounts: impl IntoIterator<Item = AccountMeta>,
        additional_signers: Vec<&Keypair>,
    ) -> Transaction {
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

        Transaction::new(
            &signers,
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&self.payer.pubkey()),
            ),
            self.svm.latest_blockhash(),
        )
    }

    pub fn cancel_intent(
        &mut self,
        intent_hash: Bytes32,
        route: &Route,
        reward_hash: Bytes32,
        fulfill_marker: Pubkey,
    ) -> TransactionResult {
        self.cancel_intent_with_call_accounts(
            intent_hash,
            route,
            reward_hash,
            fulfill_marker,
            vec![],
        )
    }

    /// `route` is the compact route `fulfill` takes; `call_accounts` are the
    /// canonical call account metas, whose flags travel in `account_flags`.
    pub fn cancel_intent_with_call_accounts(
        &mut self,
        intent_hash: Bytes32,
        route: &Route,
        reward_hash: Bytes32,
        fulfill_marker: Pubkey,
        call_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let transaction = self.cancel_intent_transaction(
            intent_hash,
            route,
            reward_hash,
            fulfill_marker,
            call_accounts,
        );

        self.send_transaction(transaction)
    }

    pub fn cancel_intent_transaction(
        &self,
        intent_hash: Bytes32,
        route: &Route,
        reward_hash: Bytes32,
        fulfill_marker: Pubkey,
        call_accounts: Vec<AccountMeta>,
    ) -> Transaction {
        let account_flags = call_accounts
            .iter()
            .map(|meta| {
                (if meta.is_signer {
                    portal::instructions::ACCOUNT_FLAG_SIGNER
                } else {
                    0
                }) | (if meta.is_writable {
                    portal::instructions::ACCOUNT_FLAG_WRITABLE
                } else {
                    0
                })
            })
            .collect();
        let args = portal::instructions::CancelArgs {
            intent_hash,
            route: route.clone(),
            reward_hash,
            account_flags,
        };
        let instruction = Instruction {
            program_id: portal::ID,
            accounts: portal::accounts::Cancel {
                payer: self.payer.pubkey(),
                fulfill_marker,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None)
            .into_iter()
            .chain(
                call_accounts
                    .iter()
                    .map(|meta| AccountMeta::new_readonly(meta.pubkey, false)),
            )
            .collect(),
            data: portal::instruction::Cancel { args }.data(),
        };

        Transaction::new(
            &[&self.payer],
            Message::new(
                &[
                    ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT),
                    instruction,
                ],
                Some(&self.payer.pubkey()),
            ),
            self.svm.latest_blockhash(),
        )
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
                AccountMeta::new_readonly(super::SPL_NOOP_ID, false),
                AccountMeta::new_readonly(unique_message.pubkey(), true),
                AccountMeta::new(dispatched_message_pda, false),
                AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
                AccountMeta::new_readonly(mailbox_program, false),
            ],
            None,
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
                AccountMeta::new_readonly(super::SPL_NOOP_ID, false),
                AccountMeta::new_readonly(unique_message.pubkey(), true),
                AccountMeta::new(dispatched_message_pda, false),
                AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
                AccountMeta::new_readonly(mailbox_program, false),
            ],
            None,
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
            None,
        )
    }

    /// Drives `portal::prove` with a caller-chosen `prover` program and a
    /// fully attacker-controlled remaining-account list — the degrees of
    /// freedom a real caller has. Used to reproduce the confused-deputy
    /// prover-delegation exploit.
    #[allow(clippy::too_many_arguments)]
    pub fn prove_intent_via_program(
        &mut self,
        prover: Pubkey,
        intent_hashes: Vec<Bytes32>,
        source_chain_domain_id: u64,
        fulfill_markers: Vec<Pubkey>,
        dispatcher: Pubkey,
        data: Vec<u8>,
        remaining_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        self.prove_intent(
            intent_hashes,
            prover,
            source_chain_domain_id,
            fulfill_markers,
            dispatcher,
            data,
            vec![],
            remaining_accounts,
            None,
        )
    }

    /// Same as [`Self::prove_intent_via_program`] but prepends
    /// `ComputeBudgetInstruction::set_compute_unit_limit(compute_unit_limit)`,
    /// for batches too large for the default per-instruction budget.
    #[allow(clippy::too_many_arguments)]
    pub fn prove_intent_via_program_with_compute_limit(
        &mut self,
        prover: Pubkey,
        intent_hashes: Vec<Bytes32>,
        source_chain_domain_id: u64,
        fulfill_markers: Vec<Pubkey>,
        dispatcher: Pubkey,
        data: Vec<u8>,
        remaining_accounts: Vec<AccountMeta>,
        compute_unit_limit: u32,
    ) -> TransactionResult {
        self.prove_intent(
            intent_hashes,
            prover,
            source_chain_domain_id,
            fulfill_markers,
            dispatcher,
            data,
            vec![],
            remaining_accounts,
            Some(compute_unit_limit),
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
        compute_unit_limit: Option<u32>,
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

        let instructions: Vec<_> = compute_unit_limit
            .map(ComputeBudgetInstruction::set_compute_unit_limit)
            .into_iter()
            .chain(iter::once(instruction))
            .collect();

        let key_pairs = iter::once(&self.payer)
            .chain(remaining_key_pairs.iter())
            .collect::<Vec<_>>();
        let transaction = Transaction::new(
            &key_pairs,
            Message::new(&instructions, Some(&self.payer.pubkey())),
            self.svm.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
    pub fn close_proof_instruction(
        &self,
        destination: u64,
        route_hash: Bytes32,
        reward: Reward,
        accounts: Vec<AccountMeta>,
    ) -> Instruction {
        let intent_hash = portal::types::intent_hash(destination, &route_hash, &reward.hash());
        Instruction {
            program_id: portal::ID,
            accounts: portal::accounts::CloseProof {
                withdrawn_marker: portal::state::WithdrawnMarker::pda(&intent_hash).0,
                prover: reward.prover,
                proof_closer: proof_closer_pda(&intent_hash).0,
            }
            .to_account_metas(None)
            .into_iter()
            .chain(accounts)
            .collect(),
            data: portal::instruction::CloseProof {
                args: portal::instructions::CloseProofArgs {
                    destination,
                    route_hash,
                    reward,
                },
            }
            .data(),
        }
    }

    pub fn close_proof(
        &mut self,
        destination: u64,
        route_hash: Bytes32,
        reward: Reward,
        accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let instruction = self.close_proof_instruction(destination, route_hash, reward, accounts);
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );
        self.send_transaction(transaction)
    }
}
