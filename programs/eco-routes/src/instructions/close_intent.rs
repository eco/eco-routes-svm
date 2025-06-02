use anchor_lang::prelude::*;

use crate::error::EcoRoutesError;

use crate::state::{Intent, IntentStatus};

#[derive(Accounts)]
pub struct CloseIntent<'info> {
    #[account(
        mut,
        seeds = [b"intent", intent.intent_hash.as_ref()],
        bump = intent.bump,
        constraint = matches!(intent.status, IntentStatus::Funding(false, 0) | IntentStatus::Claimed(true, _)) @ EcoRoutesError::IntentStillFunded
    )]
    pub intent: Account<'info, Intent>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn close_intent(ctx: Context<CloseIntent>) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let payer = &ctx.accounts.payer;

    match intent.status {
        IntentStatus::Claimed(true, claimed_token_count)
            if claimed_token_count == intent.reward.tokens.len() as u8 =>
        {
            intent.close(payer.to_account_info())
        }
        _ => Err(EcoRoutesError::IntentStillFunded.into()),
    }
}
