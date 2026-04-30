//! Atomic flash-fulfillment orchestrator.
//!
//! Lets a solver fulfill a same-chain intent with zero upfront capital: the
//! reward funds the fulfillment in one transaction via the sequence
//! `local_prover.prove` → `portal.withdraw` → `portal.fulfill` → sweep.
//! Living in its own program (rather than extending `local-prover`) avoids
//! Solana's reentrancy rule — `local_prover` only appears on the stack
//! inside portal's `close_proof` CPI, never twice.

use anchor_lang::prelude::*;

declare_id!("EcoFvY9tDz6kaxAQxNHga68sQm535DskDBCgKm3tziaT");

// Install a 256 KB bump allocator so flash_fulfill can actually use the
// heap space requested by `ComputeBudgetInstruction::request_heap_frame`
// on the client tx. solana-program's default `BumpAllocator` has `len`
// hardcoded to 32 KB regardless of the VM's actual heap region size, so
// complex CPI chains like ours OOM well before the real ceiling. Gated
// on `custom-heap` to match solana-program's `custom_heap_default!`
// macro — with the feature on, solana-program skips installing its
// default allocator and we win by being the only `#[global_allocator]`
// in the binary. Also gated on `not(feature = "no-entrypoint")` so that
// when another program (e.g. local-prover) depends on us for CPI types,
// our allocator doesn't bleed into their binary and conflict with theirs.
// https://github.com/solana-labs/solana/issues/32607
#[cfg(all(
    feature = "custom-heap",
    target_os = "solana",
    not(feature = "no-entrypoint"),
))]
#[global_allocator]
static ALLOCATOR: anchor_lang::solana_program::entrypoint::BumpAllocator =
    anchor_lang::solana_program::entrypoint::BumpAllocator {
        start: anchor_lang::solana_program::entrypoint::HEAP_START_ADDRESS as usize,
        len: 256 * 1024,
    };

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
