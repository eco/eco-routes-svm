use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("Ecoog459avhoPbVoNnh8616uSHixCQdPPtedqNXohLLK");

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
