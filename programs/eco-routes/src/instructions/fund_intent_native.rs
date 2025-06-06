use anchor_lang::{prelude::*, system_program};

use crate::state::Intent;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct FundIntentNativeArgs {
    pub intent_hash: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: FundIntentNativeArgs)]
pub struct FundIntentNative<'info> {
    #[account(
        mut,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(mut)]
    pub funder: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn fund_intent_native(
    ctx: Context<FundIntentNative>,
    _args: FundIntentNativeArgs,
) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let funder = &ctx.accounts.funder;

    let spendable_lamports = Intent::spendable_lamports(Rent::get()?, &intent.to_account_info());

    if spendable_lamports < intent.reward.native_amount {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: funder.to_account_info(),
                    to: intent.to_account_info(),
                },
            ),
            intent.reward.native_amount - spendable_lamports,
        )?;
    }

    intent.fund_native()
}
