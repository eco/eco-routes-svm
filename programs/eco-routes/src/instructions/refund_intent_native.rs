use anchor_lang::{prelude::*, system_program};

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

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

    #[account(mut)]
    pub refundee: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn refund_intent_native(
    ctx: Context<RefundIntentNative>,
    _args: RefundIntentNativeArgs,
) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let refundee = &ctx.accounts.refundee;

    if refundee.key() != intent.reward.creator {
        return Err(EcoRoutesError::InvalidRefundee.into());
    }

    if intent.status != IntentStatus::Funded {
        return Err(EcoRoutesError::NotFunded.into());
    }

    if !intent.is_expired()? {
        return Err(EcoRoutesError::IntentNotExpired.into());
    }

    if !intent.native_funded {
        return Err(EcoRoutesError::NotFunded.into());
    }

    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: intent.to_account_info(),
                to: refundee.to_account_info(),
            },
        ),
        intent.reward.native_amount,
    )?;

    intent.native_funded = false;

    if intent.is_empty() {
        intent.status = IntentStatus::Refunded;
    }

    Ok(())
}
