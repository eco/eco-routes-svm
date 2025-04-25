use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum TokenToClaim {
    Route(u8),
    Reward(u8),
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct ClaimIntentSplArgs {
    pub intent_hash: [u8; 32],
    pub token_to_claim: TokenToClaim,
}

#[derive(Accounts)]
#[instruction(args: ClaimIntentSplArgs)]
pub struct ClaimIntentSpl<'info> {
    #[account(
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        token::mint = mint,
        token::authority = intent,
        seeds = [match args.token_to_claim {
            TokenToClaim::Route(_) => b"routed-token",
            TokenToClaim::Reward(_) => b"reward-token",
        }, args.intent_hash.as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub source_token: InterfaceAccount<'info, TokenAccount>,

    #[account(
        token::mint = mint,
        token::authority = claimer,
    )]
    pub destination_token: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub claimer: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn claim_intent_spl(ctx: Context<ClaimIntentSpl>, args: ClaimIntentSplArgs) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let source_token = &mut ctx.accounts.source_token;
    let destination_token = &mut ctx.accounts.destination_token;
    let mint = &ctx.accounts.mint;
    let claimer = &ctx.accounts.claimer;
    let payer = &ctx.accounts.payer;
    let token_program = &ctx.accounts.token_program;

    if claimer.key() != intent.solver {
        return Err(EcoRoutesError::InvalidClaimer.into());
    }

    if intent.status != IntentStatus::Fulfilled {
        return Err(EcoRoutesError::NotFulfilled.into());
    }

    if !intent.is_expired()? {
        return Err(EcoRoutesError::IntentNotExpired.into());
    }

    let token_to_claim = match args.token_to_claim {
        TokenToClaim::Route(index) => intent
            .route
            .tokens
            .get(index as usize)
            .ok_or(EcoRoutesError::InvalidTokenIndex)?,
        TokenToClaim::Reward(index) => intent
            .reward
            .tokens
            .get(index as usize)
            .ok_or(EcoRoutesError::InvalidTokenIndex)?,
    };

    if mint.key() != token_to_claim.mint {
        return Err(EcoRoutesError::InvalidMint.into());
    }

    if destination_token.amount != token_to_claim.amount {
        return Err(EcoRoutesError::NotFunded.into());
    }

    anchor_spl::token_interface::transfer_checked(
        CpiContext::new_with_signer(
            token_program.to_account_info(),
            anchor_spl::token_interface::TransferChecked {
                from: destination_token.to_account_info(),
                mint: mint.to_account_info(),
                to: source_token.to_account_info(),
                authority: intent.to_account_info(),
            },
            &[&[
                b"intent",
                intent.salt.as_ref(),
                mint.key().as_ref(),
                &[intent.bump],
            ]],
        ),
        token_to_claim.amount,
        mint.decimals,
    )?;

    anchor_spl::token_interface::close_account(CpiContext::new_with_signer(
        token_program.to_account_info(),
        anchor_spl::token_interface::CloseAccount {
            account: destination_token.to_account_info(),
            destination: payer.to_account_info(),
            authority: intent.to_account_info(),
        },
        &[&[
            b"intent",
            intent.salt.as_ref(),
            mint.key().as_ref(),
            &[intent.bump],
        ]],
    ))?;

    match args.token_to_claim {
        TokenToClaim::Route(_) => intent.route.tokens_funded -= 1,
        TokenToClaim::Reward(_) => intent.reward.tokens_funded -= 1,
    }

    if intent.is_empty() {
        intent.status = IntentStatus::Claimed;
    }

    Ok(())
}
