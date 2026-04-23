use anchor_lang::prelude::AccountMeta;
use anchor_lang::{AnchorSerialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use derive_more::{Deref, DerefMut};
use eco_svm_std::prover::Proof;
use eco_svm_std::{event_authority_pda, Bytes32, CHAIN_ID};
use flash_fulfiller::instructions::{
    AppendFlashFulfillRouteChunkArgs, CancelFlashFulfillIntentArgs,
    CloseAbandonedFlashFulfillIntentArgs, FlashFulfillArgs, FlashFulfillIntent,
    InitFlashFulfillIntentArgs, SetFlashFulfillIntentArgs,
};
use flash_fulfiller::state::{flash_vault_pda, FlashFulfillIntentAccount};
use portal::state::{executor_pda, proof_closer_pda, vault_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{intent_hash, Reward, Route};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{Context, TransactionResult};

#[derive(Deref, DerefMut)]
pub struct FlashFulfiller<'a>(&'a mut Context);

impl Context {
    pub fn flash_fulfiller(&mut self) -> FlashFulfiller {
        FlashFulfiller(self)
    }
}

impl FlashFulfiller<'_> {
    pub fn set_flash_fulfill_intent(
        &mut self,
        writer: &Keypair,
        flash_fulfill_intent: Pubkey,
        route: Route,
        reward: Reward,
    ) -> TransactionResult {
        let args = SetFlashFulfillIntentArgs { route, reward };
        let instruction = flash_fulfiller::instruction::SetFlashFulfillIntent { args };
        let accounts = flash_fulfiller::accounts::SetFlashFulfillIntent {
            writer: writer.pubkey(),
            flash_fulfill_intent,
            system_program: anchor_lang::system_program::ID,
        };
        let instruction = Instruction {
            program_id: flash_fulfiller::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, writer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn init_flash_fulfill_intent(
        &mut self,
        writer: &Keypair,
        flash_fulfill_intent: Pubkey,
        args: InitFlashFulfillIntentArgs,
    ) -> TransactionResult {
        let instruction = flash_fulfiller::instruction::InitFlashFulfillIntent { args };
        let accounts = flash_fulfiller::accounts::InitFlashFulfillIntent {
            writer: writer.pubkey(),
            flash_fulfill_intent,
            system_program: anchor_lang::system_program::ID,
        };
        let instruction = Instruction {
            program_id: flash_fulfiller::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, writer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn append_flash_fulfill_route_chunk(
        &mut self,
        writer: &Keypair,
        flash_fulfill_intent: Pubkey,
        args: AppendFlashFulfillRouteChunkArgs,
    ) -> TransactionResult {
        let instruction = flash_fulfiller::instruction::AppendFlashFulfillRouteChunk { args };
        let accounts = flash_fulfiller::accounts::AppendFlashFulfillRouteChunk {
            writer: writer.pubkey(),
            flash_fulfill_intent,
        };
        let instruction = Instruction {
            program_id: flash_fulfiller::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, writer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn cancel_flash_fulfill_intent(
        &mut self,
        writer: &Keypair,
        flash_fulfill_intent: Pubkey,
        intent_hash: Bytes32,
    ) -> TransactionResult {
        let args = CancelFlashFulfillIntentArgs { intent_hash };
        let instruction = flash_fulfiller::instruction::CancelFlashFulfillIntent { args };
        let accounts = flash_fulfiller::accounts::CancelFlashFulfillIntent {
            writer: writer.pubkey(),
            flash_fulfill_intent,
        };
        let instruction = Instruction {
            program_id: flash_fulfiller::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        let transaction = Transaction::new(
            &[&self.payer, writer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn close_abandoned_flash_fulfill_intent(
        &mut self,
        caller: &Keypair,
        writer: Pubkey,
        flash_fulfill_intent: Pubkey,
        intent_hash: Bytes32,
    ) -> TransactionResult {
        let args = CloseAbandonedFlashFulfillIntentArgs { intent_hash };
        let instruction =
            flash_fulfiller::instruction::CloseAbandonedFlashFulfillIntent { args };
        let accounts = flash_fulfiller::accounts::CloseAbandonedFlashFulfillIntent {
            caller: caller.pubkey(),
            writer,
            flash_fulfill_intent,
        };
        let instruction = Instruction {
            program_id: flash_fulfiller::ID,
            accounts: accounts.to_account_metas(None),
            data: instruction.data(),
        };
        // Caller pays the tx fee so writer↔caller rent accounting in tests is clean.
        let transaction = Transaction::new(
            &[caller],
            Message::new(&[instruction], Some(&caller.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    /// Runs init + one full-payload append across two separate transactions,
    /// exercising the chunked-buffer flow without needing multi-chunk setup.
    pub fn init_and_append_full(
        &mut self,
        writer: &Keypair,
        route: &Route,
        reward: &Reward,
    ) -> TransactionResult {
        let route_hash = route.hash();
        let reward_hash = reward.hash();
        let intent_hash_value = intent_hash(CHAIN_ID, &route_hash, &reward_hash);
        let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value, &writer.pubkey()).0;

        let route_bytes = route.try_to_vec().unwrap();
        let route_total_size = route_bytes.len() as u32;

        self.init_flash_fulfill_intent(
            writer,
            buffer,
            InitFlashFulfillIntentArgs {
                intent_hash: intent_hash_value,
                route_hash,
                reward: reward.clone(),
                route_total_size,
            },
        )?;

        self.append_flash_fulfill_route_chunk(
            writer,
            buffer,
            AppendFlashFulfillRouteChunkArgs {
                intent_hash: intent_hash_value,
                offset: 0,
                chunk: route_bytes,
            },
        )
    }

    pub fn flash_fulfill(
        &mut self,
        intent: FlashFulfillIntent,
        route: &Route,
        reward: &Reward,
        claimant: Pubkey,
        claimant_atas: Vec<AccountMeta>,
        call_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let flash_vault = flash_vault_pda().0;
        let intent_vault = vault_pda(&intent_hash(CHAIN_ID, &route.hash(), &reward.hash())).0;
        let executor = executor_pda().0;
        let token_program = self.token_program;

        let reward_accounts = reward
            .tokens
            .iter()
            .flat_map(|token| {
                [
                    AccountMeta::new(
                        get_associated_token_address_with_program_id(
                            &intent_vault,
                            &token.token,
                            &token_program,
                        ),
                        false,
                    ),
                    AccountMeta::new(
                        get_associated_token_address_with_program_id(
                            &flash_vault,
                            &token.token,
                            &token_program,
                        ),
                        false,
                    ),
                    AccountMeta::new_readonly(token.token, false),
                ]
            })
            .collect();
        let route_accounts = route
            .tokens
            .iter()
            .flat_map(|token| {
                [
                    AccountMeta::new(
                        get_associated_token_address_with_program_id(
                            &flash_vault,
                            &token.token,
                            &token_program,
                        ),
                        false,
                    ),
                    AccountMeta::new(
                        get_associated_token_address_with_program_id(
                            &executor,
                            &token.token,
                            &token_program,
                        ),
                        false,
                    ),
                    AccountMeta::new_readonly(token.token, false),
                ]
            })
            .collect();

        self.flash_fulfill_with_accounts(
            intent,
            route,
            reward,
            claimant,
            reward_accounts,
            route_accounts,
            claimant_atas,
            call_accounts,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn flash_fulfill_with_accounts(
        &mut self,
        intent: FlashFulfillIntent,
        route: &Route,
        reward: &Reward,
        claimant: Pubkey,
        reward_accounts: Vec<AccountMeta>,
        route_accounts: Vec<AccountMeta>,
        claimant_atas: Vec<AccountMeta>,
        call_accounts: Vec<AccountMeta>,
    ) -> TransactionResult {
        let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
        // Buffer PDA is derived using the fee payer as the writer. Existing
        // callers always use `ctx.payer` as the writer for IntentHash tests.
        let flash_fulfill_intent = match &intent {
            FlashFulfillIntent::IntentHash(_) => {
                Some(FlashFulfillIntentAccount::pda(&intent_hash_value, &self.payer.pubkey()).0)
            }
            FlashFulfillIntent::Intent { .. } => None,
        };

        let accounts = flash_fulfiller::accounts::FlashFulfill {
            payer: self.payer.pubkey(),
            flash_vault: flash_vault_pda().0,
            flash_fulfill_intent,
            claimant,
            proof: Proof::pda(&intent_hash_value, &local_prover::ID).0,
            intent_vault: vault_pda(&intent_hash_value).0,
            withdrawn_marker: WithdrawnMarker::pda(&intent_hash_value).0,
            proof_closer: proof_closer_pda().0,
            executor: executor_pda().0,
            fulfill_marker: FulfillMarker::pda(&intent_hash_value).0,
            portal_program: portal::ID,
            local_prover_program: local_prover::ID,
            local_prover_event_authority: event_authority_pda(&local_prover::ID).0,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
            event_authority: event_authority_pda(&flash_fulfiller::ID).0,
            program: flash_fulfiller::ID,
        };
        let instruction_data = flash_fulfiller::instruction::FlashFulfill {
            args: FlashFulfillArgs { intent },
        };

        let account_metas: Vec<AccountMeta> = accounts
            .to_account_metas(None)
            .into_iter()
            .chain(reward_accounts)
            .chain(route_accounts)
            .chain(claimant_atas)
            .chain(call_accounts)
            .collect();

        let instruction = Instruction {
            program_id: flash_fulfiller::ID,
            accounts: account_metas,
            data: instruction_data.data(),
        };
        let compute_budget = ComputeBudgetInstruction::set_compute_unit_limit(1_000_000);
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[compute_budget, instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}
