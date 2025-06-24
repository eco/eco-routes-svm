use std::collections::{BTreeMap, BTreeSet};

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::{token, token_2022};
use eco_svm_std::account::AccountExt;
use eco_svm_std::{Bytes32, Proof};

use crate::events::IntentWithdrawn;
use crate::instructions::PortalError;
use crate::state::{vault_pda, WithdrawnMarker, CLAIMED_MARKER_SEED, VAULT_SEED};
use crate::types::{self, Reward, TokenTransferAccounts, VecTokenTransferAccounts};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct WithdrawArgs {
    pub destination_chain: Bytes32,
    pub route_hash: Bytes32,
    pub reward: Reward,
}

#[derive(Accounts)]
#[instruction(args: WithdrawArgs)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: validated in `validate_proof`
    #[account(mut)]
    pub claimant: UncheckedAccount<'info>,
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

pub fn withdraw_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, Withdraw<'info>>,
    args: WithdrawArgs,
) -> Result<()> {
    let WithdrawArgs {
        destination_chain,
        route_hash,
        reward,
    } = args;
    let intent_hash = types::intent_hash(&destination_chain, &route_hash, &reward.hash());
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
    validate_proof(
        &ctx.accounts.proof,
        &ctx.accounts.claimant,
        destination_chain,
    )?;

    withdraw_native(&ctx, &reward, &signer_seeds)?;
    withdraw_tokens(&ctx, &reward, &signer_seeds)?;

    // once initialized, withdraw is never allowed again
    mark_withdrawn(&ctx, &intent_hash)?;

    emit!(IntentWithdrawn::new(
        intent_hash,
        ctx.accounts.claimant.key()
    ));

    Ok(())
}

fn validate_proof(
    proof: &AccountInfo,
    claimant: &AccountInfo,
    destination_chain: Bytes32,
) -> Result<()> {
    match Proof::try_from_account_info(proof)? {
        Some(proof)
            if proof.claimant == claimant.key() && proof.destination_chain == destination_chain =>
        {
            Ok(())
        }
        _ => Err(PortalError::IntentNotFulfilled.into()),
    }
}

fn withdraw_native<'info>(
    ctx: &Context<'_, '_, '_, 'info, Withdraw<'info>>,
    reward: &Reward,
    signer_seeds: &[&[u8]],
) -> Result<()> {
    match reward.native_amount.min(ctx.accounts.vault.lamports()) {
        0 => Ok(()),
        amount => invoke_signed(
            &system_instruction::transfer(
                &ctx.accounts.vault.key(),
                &ctx.accounts.claimant.key(),
                amount,
            ),
            &[
                ctx.accounts.vault.to_account_info(),
                ctx.accounts.claimant.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            &[signer_seeds],
        )
        .map_err(Into::into),
    }
}

fn withdraw_tokens<'info>(
    ctx: &Context<'_, '_, '_, 'info, Withdraw<'info>>,
    reward: &Reward,
    signer_seeds: &[&[u8]],
) -> Result<()> {
    let accounts: VecTokenTransferAccounts<'info> = ctx.remaining_accounts.try_into()?;
    let accounts = accounts.into_inner();
    let mints = accounts
        .iter()
        .map(|accounts| accounts.mint.key())
        .collect::<BTreeSet<_>>();
    let reward_token_amounts = reward.token_amounts()?;

    require!(
        mints.len() == accounts.len() && mints.iter().eq(reward_token_amounts.keys()),
        PortalError::InvalidMint
    );

    accounts
        .into_iter()
        .try_for_each(|accounts| withdraw_token(ctx, &reward_token_amounts, signer_seeds, accounts))
}

fn withdraw_token<'info>(
    ctx: &Context<'_, '_, '_, 'info, Withdraw<'info>>,
    reward_token_amounts: &BTreeMap<Pubkey, u64>,
    signer_seeds: &[&[u8]],
    accounts: TokenTransferAccounts<'info>,
) -> Result<()> {
    let mint_key = accounts.mint.key();
    let vault_ata = get_associated_token_address_with_program_id(
        ctx.accounts.vault.key,
        &mint_key,
        accounts.token_program_id(),
    );

    require!(accounts.from.key() == vault_ata, PortalError::InvalidAta);
    require!(
        accounts.to_data()?.owner == ctx.accounts.claimant.key(),
        PortalError::InvalidClaimantToken
    );
    let reward_token_amount = *reward_token_amounts
        .get(&mint_key)
        .ok_or(PortalError::InvalidMint)?;
    let token_program = accounts.token_program(
        &ctx.accounts.token_program,
        &ctx.accounts.token_2022_program,
    )?;
    let amount = reward_token_amount.min(accounts.from_data()?.amount);

    accounts.transfer_with_signer(&token_program, &ctx.accounts.vault, &[signer_seeds], amount)?;

    Ok(())
}

fn mark_withdrawn<'info>(
    ctx: &Context<'_, '_, '_, 'info, Withdraw<'info>>,
    intent_hash: &Bytes32,
) -> Result<()> {
    let (withdrawn_marker_pda, bump) = WithdrawnMarker::pda(intent_hash);
    require!(
        ctx.accounts.withdrawn_marker.key() == withdrawn_marker_pda,
        PortalError::InvalidWithdrawnMarker
    );
    let signer_seeds = [CLAIMED_MARKER_SEED, intent_hash.as_ref(), &[bump]];

    WithdrawnMarker::default()
        .init(
            &ctx.accounts.withdrawn_marker,
            &ctx.accounts.payer,
            &ctx.accounts.system_program,
            &signer_seeds,
        )
        .map_err(|_| PortalError::IntentAlreadyWithdrawn.into())
}
