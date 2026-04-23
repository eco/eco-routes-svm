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

pub mod cpi;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod flash_fulfiller {
    use super::*;

    /// Stores a `(route, reward)` pair under the intent-hash PDA so later
    /// `flash_fulfill` calls can reference the intent by hash only.
    pub fn set_flash_fulfill_intent(
        ctx: Context<SetFlashFulfillIntent>,
        args: SetFlashFulfillIntentArgs,
    ) -> Result<()> {
        instructions::set_flash_fulfill_intent(ctx, args)
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
