use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::CHAIN_ID;
use portal::types::{intent_hash, Reward, Route};

use crate::instructions::FlashFulfillerError;
use crate::state::{FlashFulfillIntentAccount, FLASH_FULFILL_INTENT_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SetFlashFulfillIntentArgs {
    pub route: Route,
    pub reward: Reward,
}

#[derive(Accounts)]
pub struct SetFlashFulfillIntent<'info> {
    #[account(mut)]
    pub writer: Signer<'info>,
    /// CHECK: address + init handled in handler
    #[account(mut)]
    pub flash_fulfill_intent: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn set_flash_fulfill_intent(
    ctx: Context<SetFlashFulfillIntent>,
    args: SetFlashFulfillIntentArgs,
) -> Result<()> {
    let SetFlashFulfillIntentArgs { route, reward } = args;
    let intent_hash = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let (expected_pda, bump) = FlashFulfillIntentAccount::pda(&intent_hash);

    require!(
        ctx.accounts.flash_fulfill_intent.key() == expected_pda,
        FlashFulfillerError::InvalidFlashFulfillIntentAccount
    );

    let signer_seeds = [FLASH_FULFILL_INTENT_SEED, intent_hash.as_ref(), &[bump]];

    FlashFulfillIntentAccount {
        writer: ctx.accounts.writer.key(),
        route,
        reward,
    }
    .init(
        &ctx.accounts.flash_fulfill_intent,
        &ctx.accounts.writer,
        &ctx.accounts.system_program,
        &[&signer_seeds],
    )
}
