use anchor_lang::prelude::*;

declare_id!("52gVFYqekRiSUxWwCKPNKw9LhBsVxbZiLSnGVsTBGh5F");

pub mod events;
pub mod instructions;
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
