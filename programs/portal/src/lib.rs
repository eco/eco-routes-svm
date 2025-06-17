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
}
