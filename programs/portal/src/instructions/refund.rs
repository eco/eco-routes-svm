use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::token_interface::{close_account, CloseAccount};
use anchor_spl::{token, token_2022};
use eco_svm_std::prover::Proof;
use eco_svm_std::Bytes32;

use crate::events::IntentRefunded;
use crate::instructions::PortalError;
use crate::state::{vault_pda, WithdrawnMarker, VAULT_SEED};
use crate::types::{self, Reward, TokenTransferAccounts, VecTokenTransferAccounts};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RefundArgs {
    pub destination_chain: u64,
    pub route_hash: Bytes32,
    pub reward: Reward,
}

#[derive(Accounts)]
#[instruction(args: RefundArgs)]
pub struct Refund<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address is validated
    #[account(mut, address = args.reward.creator @ PortalError::InvalidCreator)]
    pub creator: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub vault: UncheckedAccount<'info>,
    /// CHECK: address is validated
    pub proof: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub withdrawn_marker: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub system_program: Program<'info, System>,
}

pub fn refund_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, Refund<'info>>,
    args: RefundArgs,
) -> Result<()> {
    let RefundArgs {
        destination_chain,
        route_hash,
        reward,
    } = args;
    let intent_hash = types::intent_hash(destination_chain, &route_hash, &reward.hash());
    let (vault_pda, bump) = vault_pda(&intent_hash);
    let signer_seeds = [VAULT_SEED, intent_hash.as_ref(), &[bump]];

    require!(
        ctx.accounts.vault.key() == vault_pda,
        PortalError::InvalidVault
    );
    require!(
        ctx.accounts.proof.key() == Proof::pda(&intent_hash, &reward.prover).0,
        PortalError::InvalidProof
    );
    require!(
        ctx.accounts.withdrawn_marker.key() == WithdrawnMarker::pda(&intent_hash).0,
        PortalError::InvalidWithdrawnMarker
    );

    validate_intent_status(&ctx, &reward, destination_chain)?;

    refund_native(&ctx, &signer_seeds)?;
    refund_tokens(&ctx, &signer_seeds)?;

    emit!(IntentRefunded::new(intent_hash, reward.creator));

    Ok(())
}

// TODO: allow early recover if the token specified is not a reward token (before anything)
fn validate_intent_status<'info>(
    ctx: &Context<'_, '_, '_, 'info, Refund<'info>>,
    reward: &Reward,
    destination_chain: u64,
) -> Result<()> {
    // already withdrawn
    if !ctx.accounts.withdrawn_marker.data_is_empty() {
        return Ok(());
    }

    // fulfilled but not withdrawn
    require!(
        !is_fulfilled(&ctx.accounts.proof.to_account_info(), destination_chain)?,
        PortalError::IntentFulfilledAndNotWithdrawn
    );

    // not fulfilled and not expired
    require!(
        reward.deadline <= Clock::get()?.unix_timestamp,
        PortalError::RewardNotExpired
    );

    Ok(())
}

fn is_fulfilled(proof: &AccountInfo, destination_chain: u64) -> Result<bool> {
    Ok(Proof::try_from_account_info(proof)?
        .map(|proof| proof.destination_chain == destination_chain)
        .unwrap_or_default())
}

fn refund_native(ctx: &Context<Refund>, signer_seeds: &[&[u8]]) -> Result<()> {
    match ctx.accounts.vault.lamports() {
        0 => Ok(()),
        amount => invoke_signed(
            &system_instruction::transfer(
                &ctx.accounts.vault.key(),
                &ctx.accounts.creator.key(),
                amount,
            ),
            &[
                ctx.accounts.vault.to_account_info(),
                ctx.accounts.creator.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            &[signer_seeds],
        )
        .map_err(Into::into),
    }
}

fn refund_tokens<'info>(
    ctx: &Context<'_, '_, '_, 'info, Refund<'info>>,
    signer_seeds: &[&[u8]],
) -> Result<()> {
    let accounts: VecTokenTransferAccounts<'info> = ctx.remaining_accounts.try_into()?;

    accounts
        .into_inner()
        .into_iter()
        .try_for_each(|accounts| refund_token(ctx, signer_seeds, accounts))
}

fn refund_token<'info>(
    ctx: &Context<'_, '_, '_, 'info, Refund<'info>>,
    signer_seeds: &[&[u8]],
    accounts: TokenTransferAccounts<'info>,
) -> Result<()> {
    require!(
        accounts.to_data()?.owner == ctx.accounts.creator.key(),
        PortalError::InvalidCreatorToken
    );

    let token_program = accounts.token_program(
        &ctx.accounts.token_program,
        &ctx.accounts.token_2022_program,
    )?;

    accounts.transfer_with_signer(
        &token_program,
        &ctx.accounts.vault,
        &[signer_seeds],
        accounts.from_data()?.amount,
    )?;

    close_account(CpiContext::new_with_signer(
        token_program,
        CloseAccount {
            account: accounts.from.to_account_info(),
            destination: ctx.accounts.payer.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        },
        &[signer_seeds],
    ))
}
