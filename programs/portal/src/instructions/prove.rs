use std::iter;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::prover::{self, IntentHashClaimant, ProofData, PROVE_DISCRIMINATOR};
use eco_svm_std::{Bytes32, CHAIN_ID};
use itertools::Itertools;

use crate::events::IntentProven;
use crate::instructions::PortalError;
use crate::state::{dispatcher_pda, fulfillment_claimant, FulfillMarker, DISPATCHER_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ProveArgs {
    pub prover: Pubkey,
    pub source_chain_domain_id: u64,
    pub intent_hashes: Vec<Bytes32>,
    pub data: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: ProveArgs)]
pub struct Prove<'info> {
    /// CHECK: address is validated
    #[account(executable, address = args.prover @ PortalError::InvalidProver)]
    pub prover: UncheckedAccount<'info>,
    /// CHECK: address is validated, scoped to the caller-chosen prover
    #[account(address = dispatcher_pda(&args.prover).0 @ PortalError::InvalidDispatcher)]
    pub dispatcher: UncheckedAccount<'info>,
}

pub fn prove_intent<'info>(ctx: Context<'info, Prove<'info>>, args: ProveArgs) -> Result<()> {
    let ProveArgs {
        prover: _,
        source_chain_domain_id,
        intent_hashes,
        data,
    } = args;

    require!(!intent_hashes.is_empty(), PortalError::EmptyIntentHashes);

    let (intent_hash_claimants, prove_accounts) =
        intent_hash_claimants_and_prove_accounts(&ctx, intent_hashes)?;

    intent_hash_claimants
        .iter()
        .for_each(|(intent_hash, claimant)| {
            emit!(IntentProven::new(*intent_hash, *claimant));
        });

    invoke_prover_prove(
        &ctx,
        source_chain_domain_id,
        intent_hash_claimants,
        prove_accounts,
        data,
    )?;

    Ok(())
}

type IntentHashAndClaimant = (Bytes32, Bytes32);

fn intent_hash_claimants_and_prove_accounts<'info>(
    ctx: &Context<'info, Prove<'info>>,
    intent_hashes: Vec<Bytes32>,
) -> Result<(Vec<IntentHashAndClaimant>, &'info [AccountInfo<'info>])> {
    require!(
        intent_hashes.len() <= ctx.remaining_accounts.len(),
        PortalError::InvalidFulfillMarker
    );
    let (fulfill_markers, prove_accounts) = ctx.remaining_accounts.split_at(intent_hashes.len());

    let intent_hash_claimants = fulfill_markers
        .iter()
        .zip(intent_hashes)
        .map(|(fulfill_marker, intent_hash)| {
            require!(
                fulfill_marker.key() == FulfillMarker::pda(&intent_hash).0,
                PortalError::InvalidFulfillMarker
            );

            Ok((intent_hash, fulfillment_claimant(fulfill_marker)?))
        })
        .try_collect()?;

    Ok((intent_hash_claimants, prove_accounts))
}

fn invoke_prover_prove<'info>(
    ctx: &Context<'info, Prove<'info>>,
    source_chain_domain_id: u64,
    intent_hash_claimants: Vec<IntentHashAndClaimant>,
    prove_accounts: &[AccountInfo<'info>],
    data: Vec<u8>,
) -> Result<()> {
    let intent_hashes_claimants = intent_hash_claimants
        .into_iter()
        .map(|(intent_hash, claimant)| IntentHashClaimant::new(intent_hash, claimant))
        .collect::<Vec<_>>();
    let proof_data = ProofData::new(CHAIN_ID, intent_hashes_claimants);
    let args = prover::ProveArgs::new(source_chain_domain_id, proof_data, data);
    let ix_data: Vec<_> = PROVE_DISCRIMINATOR
        .into_iter()
        .chain(borsh::to_vec(&args)?)
        .collect();

    let prover = ctx.accounts.prover.key();
    let (_, bump) = dispatcher_pda(&prover);
    let signer_seeds = [DISPATCHER_SEED, prover.as_ref(), &[bump]];

    let prove_account_metas = prove_accounts.iter().map(|account| AccountMeta {
        pubkey: account.key(),
        is_signer: account.is_signer,
        is_writable: account.is_writable,
    });
    let prove_account_infos = prove_accounts.iter().map(ToAccountInfo::to_account_info);

    let ix = Instruction::new_with_bytes(
        ctx.accounts.prover.key(),
        &ix_data,
        iter::once(AccountMeta::new_readonly(
            ctx.accounts.dispatcher.key(),
            true,
        ))
        .chain(prove_account_metas)
        .collect(),
    );

    invoke_signed(
        &ix,
        iter::once(ctx.accounts.dispatcher.to_account_info())
            .chain(prove_account_infos)
            .collect::<Vec<_>>()
            .as_slice(),
        &[&signer_seeds],
    )
    .map_err(Into::into)
}
