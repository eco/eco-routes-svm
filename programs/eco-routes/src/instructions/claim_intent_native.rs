use anchor_lang::prelude::*;

use crate::error::EcoRoutesError;
use crate::events;
use crate::state::{Intent, IntentStatus};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct ClaimIntentNativeArgs {
    pub intent_hash: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: ClaimIntentNativeArgs)]
pub struct ClaimIntentNative<'info> {
    #[account(
        mut,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
        constraint = matches!(intent.status, IntentStatus::Fulfilled | IntentStatus::Claimed(false, _)) @ EcoRoutesError::NotFunded,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        mut,
        constraint = intent.solver.map(Pubkey::new_from_array).is_some_and(|solver| claimer.key() == solver) @ EcoRoutesError::InvalidClaimer
    )]
    pub claimer: Signer<'info>,

    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn claim_intent_native(
    ctx: Context<ClaimIntentNative>,
    _args: ClaimIntentNativeArgs,
) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let claimer = &ctx.accounts.claimer;

    **intent.to_account_info().try_borrow_mut_lamports()? -= intent.reward.native_amount;
    **claimer.to_account_info().try_borrow_mut_lamports()? += intent.reward.native_amount;

    intent.claim_native().inspect(|_| {
        emit!(events::IntentClaimedNative::new(intent.intent_hash));
    })
}
