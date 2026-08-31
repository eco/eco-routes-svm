//! Publishes an intent whose reward amount is only known once an earlier intent
//! executes.
//!
//! # The problem
//!
//! An intent's reward amount is part of `Reward`, which is part of the intent
//! hash, which is the seed of its vault PDA. So an intent whose amount is only
//! known at execution time cannot be built, hashed, or funded ahead of time — the
//! vault address itself moves with the amount. That blocks any flow where one
//! intent's output feeds another's input, which is exactly a cross-chain swap
//! expressed as two intents:
//!
//! ```text
//! Solana                                        Solana / Base
//! ───────────────────────────────────           ─────────────────────
//! intent1  (same-chain, SVM → SVM)
//!   route.tokens  [WSOL, amount_in]
//!   route.calls
//!     [0] swap(WSOL → USDC, → escrow ATA)  ── produces an amount nobody knew
//!                            │
//! chain(order)  ─────────────┤  (its own transaction)
//!                            ├─ measures the escrow's USDC balance = amount_in
//!                            ├─ splices ceil(amount_in * scale / WAD) into the route
//!                            ├─ pushes amount_in into intent2's vault
//!                            └─ optionally CPIs portal::publish
//!                                              │
//! intent2  (SVM → SVM or SVM → Base)           ▼
//!   reward.tokens [USDC, amount_in]   solver delivers the scaled amount on the
//!   route         amount_in * scale   destination, whose calls pay the user
//! ```
//!
//! The user signs and funds only intent1.
//!
//! # Why this is a separate transaction, unlike the EVM contract
//!
//! The EVM `IntentChainer` runs *inside* intent1's fulfillment, as its last
//! `Call`: it measures, calls `Portal.publish`, and pushes into the vault address
//! that call returns — all atomically. That atomicity is the source of its best
//! property, that the intended flow never leaves a balance at rest, which is in
//! turn why it needs no access control and no per-order state.
//!
//! Solana cannot host that shape, for two independent reasons. The decisive one:
//!
//! - **Accounts are declared up front.** Intent2's vault, and its ATA, are PDAs of
//!   the intent hash, which depends on the amount. A transaction must name every
//!   account it touches before it runs, so the account the push targets cannot be
//!   known inside the transaction that discovers the amount. On EVM an address is
//!   just a value returned from a call; here it is a scheduling input.
//! - **Reentrancy.** Even setting the accounts aside, `portal::fulfill → chain →
//!   portal::publish` puts portal on the instruction stack twice, and the runtime
//!   rejects that with `ReentrancyNotAllowed` — the same rule that makes
//!   `flash-fulfiller` a separate program.
//!
//! A third constraint bounds how far the EVM shape could be pushed even if those
//! were solved: intent2's route has to be *carried* somewhere, and a realistic
//! `abi.encode(Route)` for a two-call EVM swap is ~960 bytes against the ~500 that
//! remain inside intent1's own 1232-byte transaction.
//!
//! So `chain` is its own permissionless transaction. The caller reads the escrow
//! balance, derives intent2's vault from it, and submits; the program re-measures
//! on-chain and refuses to proceed unless the accounts it was handed match what the
//! measurement implies. The measurement stays authoritative — the caller cannot
//! declare an amount that is not there — but the caller carries the scheduling.
//!
//! # What that costs, and how it is paid for
//!
//! Splitting the transaction means the balance **is** at rest between intent1's
//! fulfillment and the `chain` call, and `chain` has no signer to gate. A single
//! shared custody account would therefore let whoever calls first sweep the
//! balance into an order of their own authorship. The fix is
//! [`state::escrow_authority_pda`]: custody is seeded by
//! `keccak(borsh(order))`, so intent1 delivers into an address that encodes
//! exactly one order, that route is hash-committed, and no other order can derive
//! it. This restores the EVM's "intent1's hash authorizes the order" property
//! transitively, and it is a security boundary — read the doc comment there before
//! touching the seeds.
//!
//! Everything else follows the EVM contract closely: segments rather than write
//! offsets ([`types::Order`]), one WAD-denominated `scale` carrying both the unit
//! conversion and the solver spread, ceil rounding toward the user, a direct push
//! instead of `portal::fund`, and validate-everything-before-value-moves ordering.

use anchor_lang::prelude::*;

declare_id!("EcoiBpweBgv8guGqQd5FwERWnWBM9Knmg3NwB3PCAPj6");

pub mod events;
pub mod instructions;
mod keccak_writer;
pub mod state;
pub mod types;

use instructions::*;

#[program]
pub mod intent_chainer {
    use super::*;

    /// Records an order's preimage on-chain so the escrow it derives can always be
    /// reached again. Permissionless and account-free; see the handler docs for why
    /// it exists at all.
    pub fn announce_order(ctx: Context<AnnounceOrder>, args: AnnounceOrderArgs) -> Result<()> {
        instructions::announce_order(ctx, args)
    }

    /// Measures the escrow's balance of one mint, resolves intent2's route and
    /// reward from it, and pushes the balance into intent2's vault.
    ///
    /// Permissionless: authorization comes from the escrow authority being derived
    /// from the order's own commitment, not from a signer.
    pub fn chain<'info>(ctx: Context<'info, Chain<'info>>, args: ChainArgs) -> Result<()> {
        chain_intent(ctx, args)
    }
}

#[cfg(test)]
pub(crate) mod test_alloc {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    pub(crate) struct TrackingAllocator;

    thread_local! {
        static ALLOC_COUNT: Cell<Option<usize>> = const { Cell::new(None) };
    }

    /// Begin counting heap allocations on this thread. Resets any prior count.
    pub(crate) fn start_counting() {
        ALLOC_COUNT.with(|c| c.set(Some(0)));
    }

    /// Stop counting and return the number of allocations since `start_counting`.
    pub(crate) fn stop_counting() -> usize {
        ALLOC_COUNT.with(|c| c.take()).unwrap_or(0)
    }

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOC_COUNT.with(|c| {
                if let Some(n) = c.get() {
                    c.set(Some(n + 1));
                }
            });
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: TrackingAllocator = TrackingAllocator;
}
