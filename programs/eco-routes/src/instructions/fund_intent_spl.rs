use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum TokenToFund {
    Route(u8),
    Reward(u8),
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct FundIntentSplArgs {
    pub salt: [u8; 32],
    pub amount: u64,
    pub token_to_fund: TokenToFund,
}

#[derive(Accounts)]
#[instruction(args: FundIntentSplArgs)]
pub struct FundIntentSpl<'info> {
    #[account(
        seeds = [b"intent", args.salt.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        token::mint = mint,
        token::authority = funder,
    )]
    pub source_token: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init_if_needed,
        payer = payer,
        token::mint = mint,
        token::authority = intent,
        seeds = [match args.token_to_fund {
            TokenToFund::Route(_) => b"routed-token",
            TokenToFund::Reward(_) => b"reward-token",
        }, args.salt.as_ref(), mint.key().as_ref()],
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

    if intent.status != IntentStatus::Initialized {
        return Err(EcoRoutesError::NotInFundingPhase.into());
    }

    let token_to_fund = match args.token_to_fund {
        TokenToFund::Route(index) => intent
            .route
            .tokens
            .get(index as usize)
            .ok_or(EcoRoutesError::InvalidTokenIndex)?,
        TokenToFund::Reward(index) => intent
            .reward
            .tokens
            .get(index as usize)
            .ok_or(EcoRoutesError::InvalidTokenIndex)?,
    };

    if mint.key() != token_to_fund.mint {
        return Err(EcoRoutesError::InvalidMint.into());
    }

    if destination_token.amount == token_to_fund.amount {
        return Err(EcoRoutesError::AlreadyFunded.into());
    }

    let (amount, funded) = if destination_token.amount + args.amount >= token_to_fund.amount {
        (token_to_fund.amount - destination_token.amount, true)
    } else {
        (args.amount, false)
    };

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
        amount,
        mint.decimals,
    )?;

    if funded {
        match args.token_to_fund {
            TokenToFund::Route(_) => intent.route.tokens_funded += 1,
            TokenToFund::Reward(_) => intent.reward.tokens_funded += 1,
        }
    }

    if intent.is_funded() {
        intent.status = IntentStatus::Funded;
    }

    Ok(())
}
