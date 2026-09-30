use anchor_lang::prelude::*;
use eco_svm_std::prover::{self, cpi, GetProofArgs};
use eco_svm_std::Bytes32;

use crate::instructions::{now, PortalError};
use crate::state::{proof_closer_pda, WithdrawnMarker, PROOF_CLOSER_SEED};
use crate::types::{self, Reward};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CloseProofArgs {
    pub destination: u64,
    pub route_hash: Bytes32,
    pub reward: Reward,
    pub prover_data: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: CloseProofArgs)]
pub struct CloseProof<'info> {
    /// CHECK: canonical marker and contents are checked in the handler.
    pub withdrawn_marker: UncheckedAccount<'info>,
    /// CHECK: bound to the committed prover.
    #[account(executable, address = args.reward.prover @ PortalError::InvalidProver)]
    pub prover: UncheckedAccount<'info>,
    /// CHECK: canonical intent-scoped authority is checked in the handler.
    pub proof_closer: UncheckedAccount<'info>,
}

pub fn close_proof<'info>(
    ctx: Context<'info, CloseProof<'info>>,
    args: CloseProofArgs,
) -> Result<()> {
    let CloseProofArgs {
        destination,
        route_hash,
        reward,
        prover_data,
    } = args;
    let intent_hash = types::intent_hash(destination, &route_hash, &reward.hash());
    let (authority, bump) = proof_closer_pda(&intent_hash);
    require_keys_eq!(
        ctx.accounts.proof_closer.key(),
        authority,
        PortalError::InvalidProofCloser
    );
    if !WithdrawnMarker::exists(&ctx.accounts.withdrawn_marker, &intent_hash)? {
        require!(reward.deadline <= now()?, PortalError::RewardNotExpired);
        require!(
            cpi::get_proof(
                &ctx.accounts.prover,
                ctx.remaining_accounts,
                GetProofArgs::new(intent_hash, destination, prover_data.clone()),
            )?
            .is_some_and(|proof| proof.is_cancelled()),
            PortalError::IntentNotCancelled
        );
    }

    cpi::close_proof(
        &ctx.accounts.prover,
        &ctx.accounts.proof_closer,
        ctx.remaining_accounts,
        prover::CloseProofArgs::new(intent_hash, prover_data),
        &[&[PROOF_CLOSER_SEED, intent_hash.as_ref(), &[bump]]],
    )
}
