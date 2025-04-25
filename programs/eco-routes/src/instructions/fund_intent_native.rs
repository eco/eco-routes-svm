use anchor_lang::{prelude::*, system_program};

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum NativeToFund {
    Reward,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct FundIntentNativeArgs {
    pub salt: [u8; 32],
    pub amount: u64,
    pub native_to_fund: NativeToFund,
}

#[derive(Accounts)]
#[instruction(args: FundIntentNativeArgs)]
pub struct FundIntentNative<'info> {
    #[account(
        seeds = [b"intent", args.salt.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(mut)]
    pub funder: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn fund_intent_native(
    ctx: Context<FundIntentNative>,
    args: FundIntentNativeArgs,
) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let funder = &ctx.accounts.funder;

    if intent.status != IntentStatus::Initialized {
        return Err(EcoRoutesError::NotInFundingPhase.into());
    }

    let native_to_fund = match args.native_to_fund {
        NativeToFund::Reward => intent.reward.native_reward,
    };

    if intent.reward.native_funded >= native_to_fund {
        return Err(EcoRoutesError::AlreadyFunded.into());
    }

    let (amount, funded) = if intent.reward.native_funded + args.amount >= native_to_fund {
        (native_to_fund - intent.reward.native_funded, true)
    } else {
        (args.amount, false)
    };

    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: funder.to_account_info(),
                to: intent.to_account_info(),
            },
        ),
        amount,
    )?;

    if funded {
        match args.native_to_fund {
            NativeToFund::Reward => intent.reward.native_funded += amount,
        }
    }

    if intent.is_funded() {
        intent.status = IntentStatus::Funded;
    }

    Ok(())
}
