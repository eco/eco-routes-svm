use anchor_lang::prelude::*;
use eco_svm_std::prover::ProveArgs;
use eco_svm_std::Bytes32;

use crate::hyperlane;
use crate::instructions::HyperProverError;
use crate::state::{dispatcher_pda, DISPATCHER_SEED};

#[derive(Accounts)]
#[instruction(args: ProveArgs)]
pub struct Prove<'info> {
    #[account(address = portal::state::dispatcher_pda().0 @ HyperProverError::InvalidPortalDispatcher)]
    pub portal_dispatcher: Signer<'info>,
    /// CHECK: address is validated
    #[account(address = dispatcher_pda().0 @ HyperProverError::InvalidDispatcher)]
    pub dispatcher: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: Checked in CPI
    #[account(mut)]
    pub outbox_pda: UncheckedAccount<'info>,
    /// CHECK: Checked in CPI
    pub spl_noop_program: UncheckedAccount<'info>,
    pub unique_message: Signer<'info>,
    /// CHECK: Checked in CPI
    #[account(mut)]
    pub dispatched_message_pda: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address is validated
    #[account(executable, address = hyperlane::MAILBOX_ID @ HyperProverError::InvalidMailbox)]
    pub mailbox_program: UncheckedAccount<'info>,
}

pub fn prove_intent(ctx: Context<Prove>, args: ProveArgs) -> Result<()> {
    let ProveArgs {
        source,
        intent_hashes_claimants,
        data,
    } = args;

    let source_prover: Bytes32 = <[u8; 32]>::try_from(data)
        .map_err(|_| HyperProverError::InvalidData)?
        .into();
    let (_, bump) = dispatcher_pda();
    let signer_seeds = [DISPATCHER_SEED, &[bump]];

    hyperlane::dispatch_msg(
        &ctx,
        chain_to_domain(source)?,
        source_prover,
        intent_hashes_claimants.to_bytes(),
        &signer_seeds,
    )
}

// TODO: We need to maintain a map here later for the transformation.
// This works now only because we use Hyperlane's domain IDs directly as chain IDs.
fn chain_to_domain(chain: u64) -> Result<u32> {
    chain
        .try_into()
        .map_err(|_| HyperProverError::InvalidChainId.into())
}
