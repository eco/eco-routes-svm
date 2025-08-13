use std::iter;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::prover::{self, IntentHashClaimant, ProofData, PROVE_DISCRIMINATOR};
use eco_svm_std::{Bytes32, CHAIN_ID};
use itertools::Itertools;

use crate::events::IntentProven;
use crate::instructions::PortalError;
use crate::state::{dispatcher_pda, FulfillMarker, DISPATCHER_SEED};

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
    /// CHECK: address is validated
    #[account(address = dispatcher_pda().0 @ PortalError::InvalidDispatcher)]
    pub dispatcher: UncheckedAccount<'info>,
}

pub fn prove_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, Prove<'info>>,
    args: ProveArgs,
) -> Result<()> {
    let ProveArgs {
        prover: _,
        source_chain_domain_id,
        intent_hashes,
        data,
    } = args;

    require!(!intent_hashes.is_empty(), PortalError::EmptyIntentHashes);

    let (intent_hashes_fulfill_markers, prove_accounts) =
        fulfill_marker_and_prove_accounts(&ctx, intent_hashes)?;

    intent_hashes_fulfill_markers
        .iter()
        .for_each(|(intent_hash, fulfill_marker)| {
            emit!(IntentProven::new(*intent_hash, fulfill_marker.claimant));
        });

    invoke_prover_prove(
        &ctx,
        source_chain_domain_id,
        intent_hashes_fulfill_markers,
        prove_accounts,
        data,
    )?;

    Ok(())
}

type IntentHashAndFulfillMarker = (Bytes32, FulfillMarker);

fn fulfill_marker_and_prove_accounts<'c, 'info>(
    ctx: &Context<'_, '_, 'c, 'info, Prove<'info>>,
    intent_hashes: Vec<Bytes32>,
) -> Result<(Vec<IntentHashAndFulfillMarker>, &'c [AccountInfo<'info>])> {
    require!(
        intent_hashes.len() <= ctx.remaining_accounts.len(),
        PortalError::InvalidFulfillMarker
    );
    let (fulfill_markers, prove_accounts) = ctx.remaining_accounts.split_at(intent_hashes.len());

    let intent_hashes_fulfill_markers = fulfill_markers
        .iter()
        .zip(intent_hashes)
        .map(|(fulfill_marker, intent_hash)| {
            require!(
                fulfill_marker.key() == FulfillMarker::pda(&intent_hash).0,
                PortalError::InvalidFulfillMarker
            );

            Ok((
                intent_hash,
                FulfillMarker::try_deserialize(&mut &fulfill_marker.try_borrow_data()?[..])
                    .map_err(|_| PortalError::InvalidFulfillMarker)?,
            ))
        })
        .try_collect()?;

    Ok((intent_hashes_fulfill_markers, prove_accounts))
}

fn invoke_prover_prove<'info>(
    ctx: &Context<'_, '_, '_, 'info, Prove<'info>>,
    source_chain_domain_id: u64,
    intent_hashes_fulfill_markers: Vec<(Bytes32, FulfillMarker)>,
    prove_accounts: &[AccountInfo<'info>],
    data: Vec<u8>,
) -> Result<()> {
    let intent_hashes_claimants = intent_hashes_fulfill_markers
        .into_iter()
        .map(|(intent_hash, fulfill_marker)| {
            IntentHashClaimant::new(intent_hash, fulfill_marker.claimant)
        })
        .collect::<Vec<_>>();
    let proof_data = ProofData::new(CHAIN_ID, intent_hashes_claimants);
    let args = prover::ProveArgs::new(source_chain_domain_id, proof_data, data);
    let ix_data: Vec<_> = PROVE_DISCRIMINATOR
        .into_iter()
        .chain(args.try_to_vec()?)
        .collect();

    let (_, bump) = dispatcher_pda();
    let signer_seeds = [DISPATCHER_SEED, &[bump]];

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
