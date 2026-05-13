use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::instructions::{close_buffer, FlashFulfillerError};
use crate::state::FLASH_FULFILL_INTENT_SEED;

/// Args for [`close_flash_fulfill_intent`]: the intent hash identifying the
/// writer's buffer to close.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CloseFlashFulfillIntentArgs {
    /// Intent hash identifying the writer's buffer (combined with `writer` in the PDA seeds).
    pub intent_hash: Bytes32,
}

/// Accounts for [`close_flash_fulfill_intent`].
#[derive(Accounts)]
#[instruction(args: CloseFlashFulfillIntentArgs)]
pub struct CloseFlashFulfillIntent<'info> {
    #[account(mut)]
    pub writer: Signer<'info>,
    /// CHECK: address validated by seed binding + program-owned check; we
    /// manually close so a malformed (non-deserializable) buffer can still
    /// be reclaimed by its writer.
    #[account(
        mut,
        seeds = [
            FLASH_FULFILL_INTENT_SEED,
            writer.key().as_ref(),
            args.intent_hash.as_ref(),
        ],
        bump,
        owner = crate::ID @ FlashFulfillerError::InvalidFlashFulfillIntentAccount,
    )]
    pub flash_fulfill_intent: UncheckedAccount<'info>,
}

/// Closes the writer's buffer and refunds rent to `writer`. Bypasses Borsh
/// deserialization so a writer who streamed malformed bytes via `append`
/// can still reclaim the rent and retry.
///
/// Caller's transaction must prepend
/// `ComputeBudgetInstruction::request_heap_frame(256 * 1024)` — see the
/// crate-level docs (applies to every instruction in this program).
pub fn close_flash_fulfill_intent(
    ctx: Context<CloseFlashFulfillIntent>,
    _args: CloseFlashFulfillIntentArgs,
) -> Result<()> {
    close_buffer(&ctx.accounts.flash_fulfill_intent, &ctx.accounts.writer)
}
