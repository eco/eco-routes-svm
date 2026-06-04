use anchor_lang::prelude::*;

declare_id!("BhZtMCFgqQSzZgkUmq4WuAvCkk7cZnNDUMDQmkL7KwEi");

pub mod events;
pub mod instructions;
mod keccak_writer;
pub mod state;
pub mod types;

use instructions::*;

#[program]
pub mod portal {
    use super::*;

    pub fn publish(ctx: Context<Publish>, args: PublishArgs) -> Result<()> {
        publish_intent(ctx, args)
    }

    pub fn fund<'info>(ctx: Context<'_, '_, '_, 'info, Fund<'info>>, args: FundArgs) -> Result<()> {
        fund_intent(ctx, args)
    }

    pub fn refund<'info>(
        ctx: Context<'_, '_, '_, 'info, Refund<'info>>,
        args: RefundArgs,
    ) -> Result<()> {
        refund_intent(ctx, args)
    }

    pub fn withdraw<'info>(
        ctx: Context<'_, '_, '_, 'info, Withdraw<'info>>,
        args: WithdrawArgs,
    ) -> Result<()> {
        withdraw_intent(ctx, args)
    }

    pub fn fulfill<'info>(
        ctx: Context<'_, '_, '_, 'info, Fulfill<'info>>,
        args: FulfillArgs,
    ) -> Result<()> {
        fulfill_intent(ctx, args)
    }

    pub fn prove<'info>(
        ctx: Context<'_, '_, '_, 'info, Prove<'info>>,
        args: ProveArgs,
    ) -> Result<()> {
        prove_intent(ctx, args)
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
