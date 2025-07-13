use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("34pNy1Kn6VzTrEK8fg1z24fknE8r1EYncASV7wQh1x6j");

pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod local_prover {

    use super::*;

    pub fn prove<'info>(
        ctx: Context<'_, '_, '_, 'info, Prove<'info>>,
        args: prover::ProveArgs,
    ) -> Result<()> {
        prove_intent(ctx, args)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
