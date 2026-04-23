use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::instructions::FlashFulfillerError;
use crate::state::{FlashFulfillIntentAccount, ABANDON_TTL_SECS, FLASH_FULFILL_INTENT_SEED};

/// Args for [`close_abandoned_flash_fulfill_intent`].
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CloseAbandonedFlashFulfillIntentArgs {
    /// Intent hash bound to the buffer PDA; used in seed derivation.
    pub intent_hash: Bytes32,
}

/// Accounts for [`close_abandoned_flash_fulfill_intent`].
#[derive(Accounts)]
#[instruction(args: CloseAbandonedFlashFulfillIntentArgs)]
pub struct CloseAbandonedFlashFulfillIntent<'info> {
    /// Any signer may invoke.
    pub caller: Signer<'info>,
    /// CHECK: must be the buffer's original writer; bound via the seeds
    /// constraint on `flash_fulfill_intent` (PDA only matches if the writer
    /// pubkey here hashes to the stored buffer's PDA).
    #[account(mut)]
    pub writer: UncheckedAccount<'info>,
    #[account(
        mut,
        seeds = [
            FLASH_FULFILL_INTENT_SEED,
            args.intent_hash.as_ref(),
            writer.key().as_ref(),
        ],
        bump,
        close = writer,
    )]
    pub flash_fulfill_intent: Account<'info, FlashFulfillIntentAccount>,
}

/// Permissionless close of an un-finalized buffer after its abandonment TTL
/// has elapsed. The supplied `writer` account must match the original writer
/// — enforced via seed derivation — and receives the rent refund, so this
/// escape hatch cannot be used to steal rent.
pub fn close_abandoned_flash_fulfill_intent(
    ctx: Context<CloseAbandonedFlashFulfillIntent>,
    _args: CloseAbandonedFlashFulfillIntentArgs,
) -> Result<()> {
    let buffer = &ctx.accounts.flash_fulfill_intent;

    require!(
        !buffer.finalized,
        FlashFulfillerError::BufferAlreadyFinalized
    );

    let now = Clock::get()?.unix_timestamp;
    require!(
        now >= buffer.created_at.saturating_add(ABANDON_TTL_SECS),
        FlashFulfillerError::NotAbandonedYet
    );

    Ok(())
}
