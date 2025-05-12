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

    // the check is against a variable number of reward tokens so we cannot use matches! in our constraint
    if let IntentStatus::Claimed(false, claimed_token_count) = intent.status {
        if claimed_token_count != intent.reward.tokens.len() as u8 {
            return Err(EcoRoutesError::IntentStillFunded.into());
        }
    }

    intent.close(payer.to_account_info())?;

    Ok(())
}
