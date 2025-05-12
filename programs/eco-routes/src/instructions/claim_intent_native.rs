use anchor_lang::prelude::*;

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

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
        constraint = intent.status == IntentStatus::Fulfilled @ EcoRoutesError::NotFulfilled,
        constraint = intent.native_funded @ EcoRoutesError::NotFunded,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        mut,
        constraint = claimer.key() == Pubkey::new_from_array(intent.solver.unwrap()) @ EcoRoutesError::InvalidClaimer
    )]
    pub claimer: Signer<'info>,

    #[account(mut)]
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

    intent.native_funded = false;

    if intent.is_empty() {
        intent.status = IntentStatus::Claimed;
    }

    Ok(())
}
