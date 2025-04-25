use anchor_lang::{prelude::*, system_program};

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum NativeToRefund {
    Reward,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct RefundIntentNativeArgs {
    pub intent_hash: [u8; 32],
    pub native_to_refund: NativeToRefund,
}

#[derive(Accounts)]
#[instruction(args: RefundIntentNativeArgs)]
pub struct RefundIntentNative<'info> {
    #[account(
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
    args: RefundIntentNativeArgs,
) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let refundee = &ctx.accounts.refundee;

    if refundee.key() != intent.creator {
        return Err(EcoRoutesError::InvalidRefundee.into());
    }

    if intent.status != IntentStatus::Funded {
        return Err(EcoRoutesError::NotFunded.into());
    }

    if !intent.is_expired()? {
        return Err(EcoRoutesError::IntentNotExpired.into());
    }

    let native_to_refund = match args.native_to_refund {
        NativeToRefund::Reward => intent.reward.native_reward,
    };

    if intent.reward.native_funded != native_to_refund {
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
        native_to_refund,
    )?;

    intent.reward.native_funded -= native_to_refund;

    if intent.is_empty() {
        intent.status = IntentStatus::Refunded;
    }

    Ok(())
}
