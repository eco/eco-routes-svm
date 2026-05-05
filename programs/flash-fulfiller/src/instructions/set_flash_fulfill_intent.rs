use anchor_lang::prelude::*;
use eco_svm_std::{account, CHAIN_ID};
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
    /// Pays for the buffer account rent; the buffer's PDA is seed-bound to this key.
    #[account(mut)]
    pub writer: Signer<'info>,
    /// CHECK: address + init handled in handler
    #[account(mut)]
    pub flash_fulfill_intent: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// Creates the `FlashFulfillIntentAccount` buffer at the PDA for the supplied `(route, reward)`.
///
/// Caller's transaction must prepend
/// `ComputeBudgetInstruction::request_heap_frame(256 * 1024)` — see the
/// crate-level docs (applies to every instruction in this program).
pub fn set_flash_fulfill_intent(
    ctx: Context<SetFlashFulfillIntent>,
    args: SetFlashFulfillIntentArgs,
) -> Result<()> {
    let SetFlashFulfillIntentArgs { route, reward } = args;
    let writer = ctx.accounts.writer.key();
    let intent_hash = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let (expected_pda, bump) = FlashFulfillIntentAccount::pda(&writer, &intent_hash);

    require!(
        ctx.accounts.flash_fulfill_intent.key() == expected_pda,
        FlashFulfillerError::InvalidFlashFulfillIntentAccount
    );

    let signer_seeds = [
        FLASH_FULFILL_INTENT_SEED,
        writer.as_ref(),
        intent_hash.as_ref(),
        &[bump],
    ];

    let flash_fulfill_intent = FlashFulfillIntentAccount { route, reward };

    account::create_account(
        &ctx.accounts.flash_fulfill_intent.to_account_info(),
        &ctx.accounts.writer.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        &crate::ID,
        8 + flash_fulfill_intent.try_to_vec()?.len(),
        &[&signer_seeds],
    )?;

    flash_fulfill_intent
        .try_serialize(&mut &mut ctx.accounts.flash_fulfill_intent.try_borrow_mut_data()?[..])
}
