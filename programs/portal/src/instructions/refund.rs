use std::collections::BTreeSet;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token_interface::{close_account, CloseAccount};
use anchor_spl::{token, token_2022};
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CANCELLED};

use crate::events::IntentRefunded;
use crate::instructions::close_proof::close_proof;
use crate::instructions::{now, PortalError};
use crate::state::{proof_closer_pda, vault_pda, WithdrawnMarker, VAULT_SEED};
use crate::types::{self, Reward, TokenTransferAccounts, VecTokenTransferAccounts};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RefundArgs {
    pub destination: u64,
    pub route_hash: Bytes32,
    pub reward: Reward,
    /// Number of trailing remaining accounts forwarded to the prover's
    /// `close_proof` when refunding a proven cancellation. The token chunks
    /// before them are caller-chosen, so the split cannot be derived.
    pub close_proof_account_count: u8,
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
    #[account(mut)]
    pub proof: UncheckedAccount<'info>,
    /// CHECK: address is validated, scoped to the intent's prover
    #[account(address = proof_closer_pda(&args.reward.prover).0 @ PortalError::InvalidProofCloser)]
    pub proof_closer: UncheckedAccount<'info>,
    /// CHECK: address is validated. Deliberately not `executable`: a timeout
    /// refund must work for an intent whose `reward.prover` is not a deployed
    /// program; executability only decides whether a proven cancellation can
    /// take the early path.
    #[account(address = args.reward.prover @ PortalError::InvalidProver)]
    pub prover: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub withdrawn_marker: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub system_program: Program<'info, System>,
}

/// Why a refund is allowed. Only `Cancelled` closes the proof.
#[derive(PartialEq, Eq)]
enum RefundPath {
    Withdrawn,
    Cancelled,
    Expired,
}

pub fn refund_intent<'info>(ctx: Context<'info, Refund<'info>>, args: RefundArgs) -> Result<()> {
    let RefundArgs {
        destination,
        route_hash,
        reward,
        close_proof_account_count,
    } = args;
    let intent_hash = types::intent_hash(destination, &route_hash, &reward.hash());
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

    let refund_path = validate_intent_status(&ctx, &reward, destination)?;
    let (token_transfer_accounts, close_proof_accounts) =
        token_transfer_and_close_proof_accounts(&ctx, close_proof_account_count)?;
    let token_transfer_accounts: VecTokenTransferAccounts<'info> =
        token_transfer_accounts.try_into()?;

    if refund_path == RefundPath::Cancelled {
        require_reward_mints_swept(&ctx, &reward, &token_transfer_accounts)?;
    }

    refund_native(&ctx, &signer_seeds)?;
    refund_tokens(&ctx, &signer_seeds, token_transfer_accounts)?;

    if refund_path == RefundPath::Cancelled {
        close_proof(
            &ctx.accounts.prover,
            &ctx.accounts.proof_closer,
            &ctx.accounts.proof,
            close_proof_accounts,
        )?;
    }

    emit!(IntentRefunded::new(intent_hash, reward.creator));

    Ok(())
}

// TODO: allow early recover if the token specified is not a reward token (before anything)
fn validate_intent_status<'info>(
    ctx: &Context<'info, Refund<'info>>,
    reward: &Reward,
    destination: u64,
) -> Result<RefundPath> {
    if !ctx.accounts.withdrawn_marker.data_is_empty() {
        return Ok(RefundPath::Withdrawn);
    }

    match Proof::try_from_account_info(&ctx.accounts.proof.to_account_info())? {
        // proven cancellation for this destination: refundable immediately,
        // unless `reward.prover` is no longer a program that can close the
        // proof — then the deadline still refunds it, leaving the proof open
        Some(proof) if proof.destination == destination && CANCELLED == proof.claimant => {
            if ctx.accounts.prover.executable {
                return Ok(RefundPath::Cancelled);
            }
        }
        // fulfilled but not withdrawn
        Some(proof) if proof.destination == destination => {
            return Err(PortalError::IntentFulfilledAndNotWithdrawn.into());
        }
        // no proof, or a proof for another destination
        _ => {}
    }

    require!(reward.deadline <= now()?, PortalError::RewardNotExpired);

    Ok(RefundPath::Expired)
}

/// The cancellation path closes the proof, after which only `reward.deadline`
/// makes the intent refundable again. `refund` is permissionless and sweeps only
/// the chunks it is given, so without this a caller could close the proof while
/// leaving reward tokens in the vault until the deadline. Extra, non-reward
/// mints remain allowed, as on the other paths.
fn require_reward_mints_swept(
    ctx: &Context<Refund>,
    reward: &Reward,
    accounts: &VecTokenTransferAccounts,
) -> Result<()> {
    let swept_mints = accounts
        .iter()
        .filter(|accounts| {
            accounts.from.key()
                == get_associated_token_address_with_program_id(
                    ctx.accounts.vault.key,
                    accounts.mint.key,
                    accounts.token_program_id(),
                )
        })
        .map(|accounts| accounts.mint.key())
        .collect::<BTreeSet<_>>();

    require!(
        reward
            .token_amounts()?
            .keys()
            .all(|mint| swept_mints.contains(mint)),
        PortalError::InvalidMint
    );

    Ok(())
}

type RefundRemainingAccounts<'info> = (&'info [AccountInfo<'info>], &'info [AccountInfo<'info>]);

fn token_transfer_and_close_proof_accounts<'info>(
    ctx: &Context<'info, Refund<'info>>,
    close_proof_account_count: u8,
) -> Result<RefundRemainingAccounts<'info>> {
    let split_index = ctx
        .remaining_accounts
        .len()
        .checked_sub(close_proof_account_count.into())
        .ok_or(Error::from(PortalError::InvalidTokenTransferAccounts))?;

    Ok(ctx.remaining_accounts.split_at(split_index))
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
    ctx: &Context<'info, Refund<'info>>,
    signer_seeds: &[&[u8]],
    accounts: VecTokenTransferAccounts<'info>,
) -> Result<()> {
    accounts
        .into_inner()
        .into_iter()
        .try_for_each(|accounts| refund_token(ctx, signer_seeds, accounts))
}

fn refund_token<'info>(
    ctx: &Context<'info, Refund<'info>>,
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
        token_program.key(),
        CloseAccount {
            account: accounts.from.to_account_info(),
            destination: ctx.accounts.payer.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        },
        &[signer_seeds],
    ))
}
