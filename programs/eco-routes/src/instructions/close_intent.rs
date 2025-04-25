use anchor_lang::prelude::*;

use crate::error::EcoRoutesError;

use crate::state::{Intent, IntentStatus};

#[derive(Accounts)]
pub struct CloseIntent<'info> {
    #[account(
        seeds = [b"intent", intent.salt.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn close_intent(ctx: Context<CloseIntent>) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let payer = &ctx.accounts.payer;

    if intent.status != IntentStatus::Refunded && intent.status != IntentStatus::Claimed {
        return Err(EcoRoutesError::IntentStillFunded.into());
    }

    intent.close(payer.to_account_info())?;

    Ok(())
}
