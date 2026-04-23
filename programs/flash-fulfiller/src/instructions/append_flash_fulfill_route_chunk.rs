use anchor_lang::prelude::*;
use anchor_lang::solana_program::keccak;
use eco_svm_std::Bytes32;
use portal::types::Route;

use crate::instructions::FlashFulfillerError;
use crate::state::{FlashFulfillIntentAccount, FLASH_FULFILL_INTENT_SEED};

/// Args for [`append_flash_fulfill_route_chunk`]: strict-append chunk write.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AppendFlashFulfillRouteChunkArgs {
    /// Intent hash bound to the buffer PDA; included for seed re-derivation.
    pub intent_hash: Bytes32,
    /// Byte offset within `route_bytes`; must equal current `route_bytes_written`.
    pub offset: u32,
    /// Chunk to append. Final chunk is the one that fills the buffer.
    pub chunk: Vec<u8>,
}

/// Accounts for [`append_flash_fulfill_route_chunk`].
#[derive(Accounts)]
#[instruction(args: AppendFlashFulfillRouteChunkArgs)]
pub struct AppendFlashFulfillRouteChunk<'info> {
    /// Must match the `writer` recorded at init — enforced by the seed constraint.
    pub writer: Signer<'info>,
    #[account(
        mut,
        seeds = [
            FLASH_FULFILL_INTENT_SEED,
            args.intent_hash.as_ref(),
            writer.key().as_ref(),
        ],
        bump,
    )]
    pub flash_fulfill_intent: Account<'info, FlashFulfillIntentAccount>,
}

/// Appends one chunk of Borsh-encoded Route bytes. Auto-finalizes when the
/// final chunk fills the buffer, at which point the keccak must match the
/// committed `route_hash` and the bytes must decode as a valid `Route`.
pub fn append_flash_fulfill_route_chunk(
    ctx: Context<AppendFlashFulfillRouteChunk>,
    args: AppendFlashFulfillRouteChunkArgs,
) -> Result<()> {
    write_buffer_chunk(
        &mut ctx.accounts.flash_fulfill_intent,
        args.offset,
        &args.chunk,
    )
}

/// Strict-append write into an in-memory buffer. Auto-finalizes on the chunk
/// that fills `route_bytes`, validating both keccak and Borsh decode before
/// flipping `finalized` — so no finalized buffer can ever be unconsumable.
pub(crate) fn write_buffer_chunk(
    buffer: &mut FlashFulfillIntentAccount,
    offset: u32,
    chunk: &[u8],
) -> Result<()> {
    require!(
        !buffer.finalized,
        FlashFulfillerError::BufferAlreadyFinalized
    );
    require!(
        offset == buffer.route_bytes_written,
        FlashFulfillerError::InvalidAppendOffset
    );

    let start = offset as usize;
    let end = start
        .checked_add(chunk.len())
        .ok_or(FlashFulfillerError::AppendOverflow)?;
    require!(
        end <= buffer.route_total_size as usize,
        FlashFulfillerError::AppendOverflow
    );

    buffer.route_bytes[start..end].copy_from_slice(chunk);
    buffer.route_bytes_written = end as u32;

    if buffer.route_bytes_written == buffer.route_total_size {
        let computed = Bytes32::from(keccak::hashv(&[&buffer.route_bytes]).0);
        require!(
            computed == buffer.route_hash,
            FlashFulfillerError::RouteHashMismatch
        );
        Route::try_from_slice(&buffer.route_bytes)
            .map_err(|_| FlashFulfillerError::RouteDecodeFailed)?;
        buffer.finalized = true;
    }

    Ok(())
}
