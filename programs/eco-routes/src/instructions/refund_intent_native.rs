use anchor_lang::prelude::*;

use crate::{error::EcoRoutesError, events, state::Intent};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct RefundIntentNativeArgs {
    pub intent_hash: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: RefundIntentNativeArgs)]
pub struct RefundIntentNative<'info> {
    #[account(
        mut,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        mut,
        address = intent.reward.creator @ EcoRoutesError::InvalidRefundee
    )]
    pub refundee: Signer<'info>,

    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn refund_intent_native(
    ctx: Context<RefundIntentNative>,
    _args: RefundIntentNativeArgs,
) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let refundee = &ctx.accounts.refundee;

    intent.refund_native(Clock::get()?)?;

    **intent.to_account_info().try_borrow_mut_lamports()? -= intent.reward.native_amount;
    **refundee.to_account_info().try_borrow_mut_lamports()? += intent.reward.native_amount;

    emit!(events::IntentRefundedNative::new(intent.intent_hash));

    Ok(())
}
