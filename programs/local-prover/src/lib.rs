use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("EcoLE4mTBSCZ4BwyxfBrCvX8tBRAXEyd1UfurM1CKDdV");

pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod local_prover {

    use super::*;

    pub fn prove<'info>(ctx: Context<'info, Prove<'info>>, args: prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }

    pub fn validate_proof(
        ctx: Context<ValidateProof>,
        args: prover::ValidateProofArgs,
    ) -> Result<bool> {
        instructions::validate_proof(ctx, args)
    }

    pub fn close_proof(ctx: Context<CloseProof>, intent_hash: eco_svm_std::Bytes32) -> Result<()> {
        instructions::close_proof(ctx, intent_hash)
    }
}
