use anchor_lang::prelude::*;

declare_id!("GheA8jLDXBRbUMXAVpLropHMjn2gw6yVf1aXP8uMnJvA");

pub mod instructions;

use instructions::*;

#[program]
pub mod compact_portal {
    use super::*;

    pub fn publish_and_fund<'info>(
        ctx: Context<'_, '_, '_, 'info, PublishAndFund<'info>>,
        args: PublishAndFundArgs,
    ) -> Result<()> {
        publish_and_fund_intent(ctx, args)
    }
}
