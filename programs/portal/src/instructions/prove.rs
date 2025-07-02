use std::iter;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::prover::{self, PROVE_DISCRIMINATOR};
use eco_svm_std::{Bytes32, CHAIN_ID};

use crate::events::IntentProven;
use crate::instructions::PortalError;
use crate::state::{dispatcher_pda, FulfillMarker, DISPATCHER_SEED, FULFILL_MARKER_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ProveArgs {
    pub prover: Pubkey,
    pub source_chain: u64,
    pub intent_hash: Bytes32,
    pub data: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: ProveArgs)]
pub struct Prove<'info> {
    /// CHECK: address is validated
    #[account(executable, address = args.prover @ PortalError::InvalidProver)]
    pub prover: UncheckedAccount<'info>,
    #[account(
        seeds = [FULFILL_MARKER_SEED, args.intent_hash.as_ref()],
        bump,
    )]
    pub fulfill_marker: Account<'info, FulfillMarker>,
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
        source_chain,
        intent_hash,
        data,
    } = args;

    invoke_prover_prove(&ctx, source_chain, intent_hash, data)?;

    emit!(IntentProven::new(intent_hash, source_chain, CHAIN_ID));

    Ok(())
}

fn invoke_prover_prove<'info>(
    ctx: &Context<'_, '_, '_, 'info, Prove<'info>>,
    source_chain: u64,
    intent_hash: Bytes32,
    data: Vec<u8>,
) -> Result<()> {
    let claimant = ctx.accounts.fulfill_marker.claimant;
    let args = prover::ProveArgs::new(source_chain, intent_hash, data, claimant);
    let ix_data: Vec<_> = PROVE_DISCRIMINATOR
        .into_iter()
        .chain(args.try_to_vec()?)
        .collect();

    let (_, bump) = dispatcher_pda();
    let signer_seeds = [DISPATCHER_SEED, &[bump]];

    let remaining_account_metas = ctx.remaining_accounts.iter().map(|account| AccountMeta {
        pubkey: account.key(),
        is_signer: account.is_signer,
        is_writable: account.is_writable,
    });
    let remaining_account_infos = ctx
        .remaining_accounts
        .iter()
        .map(ToAccountInfo::to_account_info);

    let ix = Instruction::new_with_bytes(
        ctx.accounts.prover.key(),
        &ix_data,
        iter::once(AccountMeta::new_readonly(
            ctx.accounts.dispatcher.key(),
            true,
        ))
        .chain(remaining_account_metas)
        .collect(),
    );

    invoke_signed(
        &ix,
        iter::once(ctx.accounts.dispatcher.to_account_info())
            .chain(remaining_account_infos)
            .collect::<Vec<_>>()
            .as_slice(),
        &[&signer_seeds],
    )
    .map_err(Into::into)
}
