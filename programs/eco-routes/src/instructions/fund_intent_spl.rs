use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::state::Intent;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct FundIntentSplArgs {
    pub intent_hash: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: FundIntentSplArgs)]
pub struct FundIntentSpl<'info> {
    #[account(
        mut,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = funder,
    )]
    pub funder_token: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init,
        payer = payer,
        token::mint = mint,
        token::authority = intent,
        seeds = [b"reward", args.intent_hash.as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    pub funder: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn fund_intent_spl(ctx: Context<FundIntentSpl>, _args: FundIntentSplArgs) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let funder_token = &mut ctx.accounts.funder_token;
    let vault = &mut ctx.accounts.vault;
    let mint = &ctx.accounts.mint;
    let funder = &ctx.accounts.funder;
    let token_program = &ctx.accounts.token_program;

    let token = intent.fund_token(mint.key().as_array())?;

    anchor_spl::token_interface::transfer_checked(
        CpiContext::new(
            token_program.to_account_info(),
            anchor_spl::token_interface::TransferChecked {
                from: funder_token.to_account_info(),
                mint: mint.to_account_info(),
                to: vault.to_account_info(),
                authority: funder.to_account_info(),
            },
        ),
        token.amount,
        mint.decimals,
    )
}
