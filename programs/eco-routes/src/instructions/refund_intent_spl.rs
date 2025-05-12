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
        constraint = matches!(intent.status, IntentStatus::Funding(_, _) | IntentStatus::Funded) @ EcoRoutesError::NotFunded,
        constraint = intent.is_expired().unwrap_or(false) @ EcoRoutesError::IntentNotExpired,
    )]
    pub intent: Account<'info, Intent>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = intent,
        seeds = [b"reward", args.intent_hash.as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    #[account(
        token::mint = mint,
        token::authority = refundee,
    )]
    pub refundee_token: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        address = intent.reward.creator @ EcoRoutesError::InvalidRefundee
    )]
    pub refundee: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn refund_intent_spl(ctx: Context<RefundIntentSpl>, args: RefundIntentSplArgs) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let vault = &mut ctx.accounts.vault;
    let refundee_token = &mut ctx.accounts.refundee_token;
    let mint = &ctx.accounts.mint;
    let payer = &ctx.accounts.payer;
    let token_program = &ctx.accounts.token_program;

    let token_to_refund = intent
        .reward
        .tokens
        .get(args.token_to_refund as usize)
        .ok_or(EcoRoutesError::InvalidTokenIndex)?;

    if mint.key() != Pubkey::new_from_array(token_to_refund.token) {
        return Err(EcoRoutesError::InvalidMint.into());
    }

    anchor_spl::token_interface::transfer_checked(
        CpiContext::new_with_signer(
            token_program.to_account_info(),
            anchor_spl::token_interface::TransferChecked {
                from: vault.to_account_info(),
                mint: mint.to_account_info(),
                to: refundee_token.to_account_info(),
                authority: intent.to_account_info(),
            },
            &[&[b"intent", intent.intent_hash.as_ref(), &[intent.bump]]],
        ),
        token_to_refund.amount - vault.amount,
        mint.decimals,
    )?;

    anchor_spl::token_interface::close_account(CpiContext::new_with_signer(
        token_program.to_account_info(),
        anchor_spl::token_interface::CloseAccount {
            account: vault.to_account_info(),
            destination: payer.to_account_info(),
            authority: intent.to_account_info(),
        },
        &[&[b"intent", intent.intent_hash.as_ref(), &[intent.bump]]],
    ))?;

    intent.refund_token()?;

    Ok(())
}
