use anchor_lang::prelude::AccountMeta;
use anchor_lang::InstructionData;
use compact_portal::instructions::PublishAndFundArgs;
use derive_more::{Deref, DerefMut};
use portal::types::{Reward, Route};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{Context, TransactionResult, COMPUTE_UNIT_LIMIT};

#[derive(Deref, DerefMut)]
pub struct CompactPortal<'a>(&'a mut Context);

impl Context {
    pub fn compact_portal(&mut self) -> CompactPortal {
        CompactPortal(self)
    }
}

impl CompactPortal<'_> {
    pub fn publish_and_fund(
        &mut self,
        destination: u64,
        route: Route,
        reward: Reward,
        vault: Pubkey,
        allow_partial: bool,
        token_transfer_accounts: impl IntoIterator<Item = AccountMeta>,
    ) -> TransactionResult {
        let args = PublishAndFundArgs {
            destination,
            route,
            reward,
            allow_partial,
        };
        let instruction = compact_portal::instruction::PublishAndFund { args };
        let accounts: Vec<_> = vec![
            AccountMeta::new_readonly(portal::ID, false),
            AccountMeta::new_readonly(self.payer.pubkey(), true),
            AccountMeta::new(self.funder.pubkey(), true),
            AccountMeta::new(vault, false),
            AccountMeta::new_readonly(anchor_spl::token::ID, false),
            AccountMeta::new_readonly(anchor_spl::token_2022::ID, false),
            AccountMeta::new_readonly(anchor_spl::associated_token::ID, false),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
        ]
        .into_iter()
        .chain(token_transfer_accounts)
        .collect();
        let instruction = Instruction {
            program_id: compact_portal::ID,
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
}
