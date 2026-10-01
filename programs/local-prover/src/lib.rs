use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("EcoLGDxv2hnDijPfHX9fAbFeMmUmGQ1XzLNUfCHyEw91");

pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod local_prover {

    use super::*;

    pub fn prove<'info>(ctx: Context<'info, Prove<'info>>, args: prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
