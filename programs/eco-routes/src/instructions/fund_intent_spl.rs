use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct FundIntentSplArgs {
    pub intent_hash: [u8; 32],
    pub token_to_fund: u8,
}

#[derive(Accounts)]
#[instruction(args: FundIntentSplArgs)]
pub struct FundIntentSpl<'info> {
    #[account(
        mut,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
        constraint = intent.status == IntentStatus::Initialized @ EcoRoutesError::NotInFundingPhase,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = funder,
    )]
    pub source_token: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init_if_needed,
        payer = payer,
        token::mint = mint,
        token::authority = intent,
        seeds = [b"reward", args.intent_hash.as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub destination_token: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub funder: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn fund_intent_spl(ctx: Context<FundIntentSpl>, args: FundIntentSplArgs) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let source_token = &mut ctx.accounts.source_token;
    let destination_token = &mut ctx.accounts.destination_token;
    let mint = &ctx.accounts.mint;
    let funder = &ctx.accounts.funder;
    let token_program = &ctx.accounts.token_program;

    let token_to_fund = intent
        .reward
        .tokens
        .get(args.token_to_fund as usize)
        .ok_or(EcoRoutesError::InvalidTokenIndex)?;

    if mint.key() != Pubkey::new_from_array(token_to_fund.token) {
        return Err(EcoRoutesError::InvalidMint.into());
    }

    if destination_token.amount == token_to_fund.amount {
        return Err(EcoRoutesError::AlreadyFunded.into());
    }

    anchor_spl::token_interface::transfer_checked(
        CpiContext::new(
            token_program.to_account_info(),
            anchor_spl::token_interface::TransferChecked {
                from: source_token.to_account_info(),
                mint: mint.to_account_info(),
                to: destination_token.to_account_info(),
                authority: funder.to_account_info(),
            },
        ),
        token_to_fund.amount,
        mint.decimals,
    )?;

    intent.tokens_funded += 1;

    if intent.is_funded() {
        intent.status = IntentStatus::Funded;
    }

    Ok(())
}
