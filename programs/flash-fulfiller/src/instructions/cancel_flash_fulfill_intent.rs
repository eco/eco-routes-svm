use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::instructions::FlashFulfillerError;
use crate::state::{FlashFulfillIntentAccount, FLASH_FULFILL_INTENT_SEED};

/// Args for [`cancel_flash_fulfill_intent`].
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CancelFlashFulfillIntentArgs {
    /// Intent hash bound to the buffer PDA; included for seed re-derivation.
    pub intent_hash: Bytes32,
}

/// Accounts for [`cancel_flash_fulfill_intent`].
#[derive(Accounts)]
#[instruction(args: CancelFlashFulfillIntentArgs)]
pub struct CancelFlashFulfillIntent<'info> {
    /// Original writer of the buffer; rent is refunded here.
    #[account(mut)]
    pub writer: Signer<'info>,
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

/// Writer-initiated close of an un-finalized buffer, refunding rent to the
/// writer. Allows aborting before the abandonment TTL elapses (e.g. wrong
/// route committed, price moved). A finalized buffer can only be consumed
/// via `flash_fulfill`, not cancelled.
pub fn cancel_flash_fulfill_intent(
    ctx: Context<CancelFlashFulfillIntent>,
    _args: CancelFlashFulfillIntentArgs,
) -> Result<()> {
    require!(
        !ctx.accounts.flash_fulfill_intent.finalized,
        FlashFulfillerError::BufferAlreadyFinalized
    );
    Ok(())
}
