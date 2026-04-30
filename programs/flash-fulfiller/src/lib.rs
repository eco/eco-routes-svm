//! Atomic flash-fulfillment orchestrator.
//!
//! Lets a solver fulfill a same-chain intent with zero upfront capital: the
//! reward funds the fulfillment in one transaction via the sequence
//! `local_prover.prove` → `portal.withdraw` → `portal.fulfill` → sweep.
//! Living in its own program (rather than extending `local-prover`) avoids
//! Solana's reentrancy rule — `local_prover` only appears on the stack
//! inside portal's `close_proof` CPI, never twice.

use anchor_lang::prelude::*;

declare_id!("2d8yK5bxGuoZssTgEa4Lj9Z5AyhmVySVDQ1JadaTLeaK");

pub mod cpi;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod flash_fulfiller {
    use super::*;

    /// Initializes a buffer at `pda(writer, intent_hash)` and writes the
    /// supplied `(route, reward)` typed body into it. Use this when the full
    /// `(route, reward)` body fits in a single transaction.
    pub fn set_flash_fulfill_intent(
        ctx: Context<SetFlashFulfillIntent>,
        args: SetFlashFulfillIntentArgs,
    ) -> Result<()> {
        instructions::set_flash_fulfill_intent(ctx, args)
    }

    /// Streams raw bytes into the writer's buffer. The first call allocates
    /// the buffer; subsequent calls realloc and append. Bytes streamed must
    /// form a valid Borsh `(route, reward)` body once concatenated —
    /// `flash_fulfill` rejects malformed buffers via deserialization.
    pub fn append_flash_fulfill_intent_chunk(
        ctx: Context<AppendFlashFulfillIntentChunk>,
        args: AppendFlashFulfillIntentChunkArgs,
    ) -> Result<()> {
        instructions::append_flash_fulfill_intent_chunk(ctx, args)
    }

    /// Closes the writer's buffer and refunds rent to the writer. Useful when
    /// a streamed buffer is abandoned without ever being consumed by `flash_fulfill`.
    pub fn close_flash_fulfill_intent(
        ctx: Context<CloseFlashFulfillIntent>,
        args: CloseFlashFulfillIntentArgs,
    ) -> Result<()> {
        instructions::close_flash_fulfill_intent(ctx, args)
    }

    /// Atomically proves, withdraws, fulfills, and sweeps leftovers to the
    /// caller-supplied claimant. The program's `flash_vault` PDA acts as
    /// transient solver/claimant; it is drained by the end of the tx.
    pub fn flash_fulfill<'info>(
        ctx: Context<'_, '_, '_, 'info, FlashFulfill<'info>>,
        args: FlashFulfillArgs,
    ) -> Result<()> {
        instructions::flash_fulfill(ctx, args)
    }
}
