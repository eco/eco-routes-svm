use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct RefundIntentSplArgs {
    pub intent_hash: [u8; 32],
    pub token_to_refund: u8,
}

#[derive(Accounts)]
#[instruction(args: RefundIntentSplArgs)]
pub struct RefundIntentSpl<'info> {
    #[account(
        mut,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = intent,
        seeds = [b"reward", args.intent_hash.as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub source_token: InterfaceAccount<'info, TokenAccount>,

    #[account(
        token::mint = mint,
        token::authority = refundee,
    )]
    pub destination_token: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub refundee: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn refund_intent_spl(ctx: Context<RefundIntentSpl>, args: RefundIntentSplArgs) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let source_token = &mut ctx.accounts.source_token;
    let destination_token = &mut ctx.accounts.destination_token;
    let mint = &ctx.accounts.mint;
    let refundee = &ctx.accounts.refundee;
    let payer = &ctx.accounts.payer;
    let token_program = &ctx.accounts.token_program;

    if refundee.key() != intent.reward.creator {
        return Err(EcoRoutesError::InvalidRefundee.into());
    }

    if intent.status != IntentStatus::Funded {
        return Err(EcoRoutesError::NotFunded.into());
    }

    if !intent.is_expired()? {
        return Err(EcoRoutesError::IntentNotExpired.into());
    }

    let token_to_refund = intent
        .reward
        .tokens
        .get(args.token_to_refund as usize)
        .ok_or(EcoRoutesError::InvalidTokenIndex)?;

    if mint.key() != Pubkey::new_from_array(token_to_refund.token) {
        return Err(EcoRoutesError::InvalidMint.into());
    }

    if destination_token.amount != token_to_refund.amount {
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
                intent.route.salt.as_ref(),
                mint.key().as_ref(),
                &[intent.bump],
            ]],
        ),
        token_to_refund.amount,
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
            intent.route.salt.as_ref(),
            mint.key().as_ref(),
            &[intent.bump],
        ]],
    ))?;

    intent.tokens_funded -= 1;

    if intent.is_empty() {
        intent.status = IntentStatus::Refunded;
    }

    Ok(())
}
