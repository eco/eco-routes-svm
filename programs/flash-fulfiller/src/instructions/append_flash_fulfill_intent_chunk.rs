use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::system_instruction;
use eco_svm_std::{account, Bytes32};

use crate::instructions::FlashFulfillerError;
use crate::state::FLASH_FULFILL_INTENT_SEED;

/// Args for [`append_flash_fulfill_intent_chunk`]: the intent hash identifying
/// the writer's buffer, plus the raw bytes to append.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AppendFlashFulfillIntentChunkArgs {
    /// Intent hash identifying the writer's buffer (combined with `writer` in the PDA seeds).
    pub intent_hash: Bytes32,
    /// Raw bytes appended to the buffer's tail. Caller is responsible for
    /// streaming the full on-chain layout across chunks: 8-byte Anchor
    /// discriminator (`FlashFulfillIntentAccount::DISCRIMINATOR`) followed
    /// by the Borsh-encoded `(route, reward)` body. `flash_fulfill` catches
    /// malformed bytes via deserialization at consume time.
    pub chunk: Vec<u8>,
}

/// Accounts for [`append_flash_fulfill_intent_chunk`].
#[derive(Accounts)]
#[instruction(args: AppendFlashFulfillIntentChunkArgs)]
pub struct AppendFlashFulfillIntentChunk<'info> {
    #[account(mut)]
    pub writer: Signer<'info>,
    /// CHECK: address validated by seed derivation; created on first call, extended on subsequent calls
    #[account(
        mut,
        seeds = [
            FLASH_FULFILL_INTENT_SEED,
            writer.key().as_ref(),
            args.intent_hash.as_ref(),
        ],
        bump,
    )]
    pub flash_fulfill_intent: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// Streams `chunk` into the writer's buffer. The first call allocates `chunk.len()`
/// bytes; subsequent calls realloc the buffer to its current length plus `chunk.len()`
/// and copy `chunk` into the trailing slice. The PDA's binding to `writer` makes
/// the (per-writer) ordering of chunks the writer's responsibility.
///
/// Caller's transaction must prepend
/// `ComputeBudgetInstruction::request_heap_frame(256 * 1024)` — see the
/// crate-level docs (applies to every instruction in this program).
pub fn append_flash_fulfill_intent_chunk(
    ctx: Context<AppendFlashFulfillIntentChunk>,
    args: AppendFlashFulfillIntentChunkArgs,
) -> Result<()> {
    let AppendFlashFulfillIntentChunkArgs { intent_hash, chunk } = args;
    let writer = ctx.accounts.writer.key();
    let bump = ctx.bumps.flash_fulfill_intent;
    let buffer = &ctx.accounts.flash_fulfill_intent;

    let signer_seeds: &[&[u8]] = &[
        FLASH_FULFILL_INTENT_SEED,
        writer.as_ref(),
        intent_hash.as_ref(),
        &[bump],
    ];

    if buffer.data_is_empty() && *buffer.owner != crate::ID {
        account::create_account(
            &buffer.to_account_info(),
            &ctx.accounts.writer.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            &crate::ID,
            chunk.len(),
            &[signer_seeds],
        )?;
        buffer.try_borrow_mut_data()?.copy_from_slice(&chunk);

        return Ok(());
    }

    let current_len = buffer.data_len();
    let new_len = current_len
        .checked_add(chunk.len())
        .ok_or(FlashFulfillerError::BufferLengthOverflow)?;
    let new_min_balance = Rent::get()?.minimum_balance(new_len);

    if let Some(top_up) = new_min_balance
        .checked_sub(buffer.lamports())
        .filter(|amount| *amount > 0)
    {
        invoke(
            &system_instruction::transfer(&ctx.accounts.writer.key(), &buffer.key(), top_up),
            &[
                ctx.accounts.writer.to_account_info(),
                buffer.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;
    }
    buffer.realloc(new_len, false)?;
    buffer.try_borrow_mut_data()?[current_len..].copy_from_slice(&chunk);

    Ok(())
}
