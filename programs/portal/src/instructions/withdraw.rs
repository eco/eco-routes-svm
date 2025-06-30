use std::collections::{BTreeMap, BTreeSet};

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::{token, token_2022};
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::{Proof, CLOSE_PROOF_DISCRIMINATOR};
use eco_svm_std::Bytes32;

use crate::events::IntentWithdrawn;
use crate::instructions::PortalError;
use crate::state::{
    proof_closer_pda, vault_pda, WithdrawnMarker, CLAIMED_MARKER_SEED, PROOF_CLOSER_SEED,
    VAULT_SEED,
};
use crate::types::{
    self, Reward, TokenTransferAccounts, VecTokenTransferAccounts,
    VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct WithdrawArgs {
    pub destination_chain: u64,
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
    #[account(mut)]
    pub proof: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(address = proof_closer_pda().0 @ PortalError::InvalidProofCloser)]
    pub proof_closer: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(executable, address = args.reward.prover @ PortalError::InvalidProver)]
    pub prover: UncheckedAccount<'info>,
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
    let intent_hash = types::intent_hash(destination_chain, &route_hash, &reward.hash());
    let (vault_pda, bump) = vault_pda(&intent_hash);
    let signer_seeds = [VAULT_SEED, intent_hash.as_ref(), &[bump]];

    require!(
        ctx.accounts.vault.key() == vault_pda,
        PortalError::InvalidVault
    );
    validate_proof(&ctx, destination_chain, &intent_hash, &reward.prover)?;

    withdraw_native(&ctx, &reward, &signer_seeds)?;
    let (token_transfer_accounts, remaining_accounts) =
        token_transfer_and_remaining_accounts(&ctx, &reward)?;
    withdraw_tokens(&ctx, &reward, &signer_seeds, token_transfer_accounts)?;

    // once initialized, withdraw is never allowed again
    mark_withdrawn(&ctx, &intent_hash)?;
    close_proof(&ctx, remaining_accounts)?;

    emit!(IntentWithdrawn::new(
        intent_hash,
        ctx.accounts.claimant.key()
    ));

    Ok(())
}

fn validate_proof(
    ctx: &Context<Withdraw>,
    destination_chain: u64,
    intent_hash: &Bytes32,
    prover: &Pubkey,
) -> Result<()> {
    require!(
        ctx.accounts.proof.key() == Proof::pda(intent_hash, prover).0,
        PortalError::InvalidProof
    );
    require!(
        ctx.accounts.proof.owner == prover,
        PortalError::InvalidProof
    );

    msg!(
        "Proof::try_from_account_info(&ctx.accounts.proof) {:?}",
        Proof::try_from_account_info(&ctx.accounts.proof)
    );

    match Proof::try_from_account_info(&ctx.accounts.proof)? {
        Some(proof)
            if proof.claimant == *ctx.accounts.claimant.key
                && proof.destination_chain == destination_chain =>
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

fn token_transfer_and_remaining_accounts<'c, 'info>(
    ctx: &Context<'_, '_, 'c, 'info, Withdraw<'info>>,
    reward: &Reward,
) -> Result<(VecTokenTransferAccounts<'info>, &'c [AccountInfo<'info>])> {
    let split_index = reward.tokens.len() * VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE;
    require!(
        split_index <= ctx.remaining_accounts.len(),
        PortalError::InvalidTokenTransferAccounts
    );

    let (token_transfer_accounts, remaining_accounts) =
        ctx.remaining_accounts.split_at(split_index);

    Ok((token_transfer_accounts.try_into()?, remaining_accounts))
}

fn withdraw_tokens<'info>(
    ctx: &Context<'_, '_, '_, 'info, Withdraw<'info>>,
    reward: &Reward,
    signer_seeds: &[&[u8]],
    accounts: VecTokenTransferAccounts<'info>,
) -> Result<()> {
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
            &[&signer_seeds],
        )
        .map_err(|_| PortalError::IntentAlreadyWithdrawn.into())
}

fn close_proof<'info>(
    ctx: &Context<'_, '_, '_, 'info, Withdraw<'info>>,
    remaining_accounts: &[AccountInfo<'info>],
) -> Result<()> {
    let (_, bump) = proof_closer_pda();
    let signer_seeds = [PROOF_CLOSER_SEED, &[bump]];

    let remaining_account_metas = remaining_accounts.iter().map(|account| AccountMeta {
        pubkey: account.key(),
        is_signer: account.is_signer,
        is_writable: account.is_writable,
    });
    let remaining_account_infos = remaining_accounts
        .iter()
        .map(ToAccountInfo::to_account_info);

    let ix = Instruction::new_with_bytes(
        ctx.accounts.prover.key(),
        &CLOSE_PROOF_DISCRIMINATOR,
        vec![
            AccountMeta::new_readonly(ctx.accounts.proof_closer.key(), true),
            AccountMeta::new(ctx.accounts.proof.key(), false),
        ]
        .into_iter()
        .chain(remaining_account_metas)
        .collect(),
    );

    invoke_signed(
        &ix,
        vec![
            ctx.accounts.proof_closer.to_account_info(),
            ctx.accounts.proof.to_account_info(),
        ]
        .into_iter()
        .chain(remaining_account_infos)
        .collect::<Vec<_>>()
        .as_slice(),
        &[&signer_seeds],
    )
    .map_err(Into::into)
}
