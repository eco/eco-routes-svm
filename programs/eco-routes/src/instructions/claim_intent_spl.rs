use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::error::EcoRoutesError;
use crate::events;
use crate::state::{Intent, IntentStatus};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct ClaimIntentSplArgs {
    pub intent_hash: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: ClaimIntentSplArgs)]
pub struct ClaimIntentSpl<'info> {
    #[account(
        mut,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
        constraint = matches!(intent.status, IntentStatus::Fulfilled | IntentStatus::Claimed(_, _)) @ EcoRoutesError::NotClaimable,
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
        mut,
        token::mint = mint,
        token::authority = claimer,
    )]
    pub claimer_token: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    #[account(constraint = intent.solver.map(Pubkey::new_from_array).is_some_and(|solver| claimer.key() == solver) @ EcoRoutesError::InvalidClaimer)]
    pub claimer: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
}

pub fn claim_intent_spl(ctx: Context<ClaimIntentSpl>, _args: ClaimIntentSplArgs) -> Result<()> {
    let intent = &mut ctx.accounts.intent;
    let vault = &mut ctx.accounts.vault;
    let claimer_token = &mut ctx.accounts.claimer_token;
    let mint = &ctx.accounts.mint;
    let payer = &ctx.accounts.payer;
    let token_program = &ctx.accounts.token_program;

    intent.claim_token(mint.key().as_array())?;

    anchor_spl::token_interface::transfer_checked(
        CpiContext::new_with_signer(
            token_program.to_account_info(),
            anchor_spl::token_interface::TransferChecked {
                from: vault.to_account_info(),
                mint: mint.to_account_info(),
                to: claimer_token.to_account_info(),
                authority: intent.to_account_info(),
            },
            &[&[b"intent", intent.intent_hash.as_ref(), &[intent.bump]]],
        ),
        vault.amount,
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

    emit!(events::IntentClaimedSpl::new(
        intent.intent_hash,
        mint.key()
    ));

    Ok(())
}
