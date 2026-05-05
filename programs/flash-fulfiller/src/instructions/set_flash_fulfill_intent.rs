use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::CHAIN_ID;
use portal::types::{intent_hash, Reward, Route};

use crate::instructions::FlashFulfillerError;
use crate::state::{FlashFulfillIntentAccount, FLASH_FULFILL_INTENT_SEED};

/// Args for [`set_flash_fulfill_intent`]: the `(route, reward)` pair to buffer under the intent-hash PDA.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SetFlashFulfillIntentArgs {
    /// Route that will be committed to the buffer.
    pub route: Route,
    /// Reward that will be committed to the buffer.
    pub reward: Reward,
}

/// Accounts for [`set_flash_fulfill_intent`].
#[derive(Accounts)]
pub struct SetFlashFulfillIntent<'info> {
    /// Pays for the buffer account rent and is recorded as the writer.
    #[account(mut)]
    pub writer: Signer<'info>,
    /// CHECK: address + init handled in handler
    #[account(mut)]
    pub flash_fulfill_intent: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// Creates the `FlashFulfillIntentAccount` buffer at the PDA for the supplied `(route, reward)`.
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
