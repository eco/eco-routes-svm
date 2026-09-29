use anchor_lang::prelude::*;
use eco_svm_std::prover::cpi;
use eco_svm_std::Bytes32;

use crate::instructions::AggregatorProverError;
use crate::state::Config;

#[derive(Accounts)]
#[instruction(intent_hash: Bytes32)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&intent_hash).0 @ AggregatorProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(address = Config::pda().0 @ AggregatorProverError::InvalidConfig)]
    pub config: Account<'info, Config>,
    /// CHECK: the executable program must belong to the configured prover set.
    #[account(executable, constraint = config.provers.contains(&prover.key()) @ AggregatorProverError::InvalidProver)]
    pub prover: UncheckedAccount<'info>,
}

pub fn close_proof<'info>(
    ctx: Context<'info, CloseProof<'info>>,
    intent_hash: Bytes32,
) -> Result<()> {
    cpi::close_proof(
        &ctx.accounts.prover,
        &ctx.accounts.portal_proof_closer,
        ctx.remaining_accounts,
        intent_hash,
        &[],
    )
}
