use anchor_lang::{prelude::*, system_program};

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum NativeToClaim {
    Reward,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct ClaimIntentNativeArgs {
    pub intent_hash: [u8; 32],
    pub native_to_claim: NativeToClaim,
}

#[derive(Accounts)]
#[instruction(args: ClaimIntentNativeArgs)]
pub struct ClaimIntentNative<'info> {
    #[account(
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(mut)]
    pub claimer: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn claim_intent_native(
    ctx: Context<ClaimIntentNative>,
    args: ClaimIntentNativeArgs,
) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let claimer = &ctx.accounts.claimer;

    if claimer.key() != intent.solver {
        return Err(EcoRoutesError::InvalidClaimer.into());
    }

    if intent.status != IntentStatus::Fulfilled {
        return Err(EcoRoutesError::NotFulfilled.into());
    }

    let native_to_claim = match args.native_to_claim {
        NativeToClaim::Reward => intent.reward.native_reward,
    };

    if intent.reward.native_funded != native_to_claim {
        return Err(EcoRoutesError::NotFunded.into());
    }

    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: intent.to_account_info(),
                to: claimer.to_account_info(),
            },
        ),
        native_to_claim,
    )?;

    intent.reward.native_funded -= native_to_claim;

    if intent.is_empty() {
        intent.status = IntentStatus::Claimed;
    }

    Ok(())
}
