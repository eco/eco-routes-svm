use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

declare_id!("Cg6hCaRjtPJNb7PPjGi9BMLmyoLCNatbVifaQffcQhLE");

pub mod instructions;
pub mod state;

use eco_svm_std::prover;
use instructions::*;

#[program]
pub mod aggregator_prover {

    use super::*;

    pub fn init<'info>(ctx: Context<'info, Init<'info>>) -> Result<()> {
        instructions::init(ctx)
    }

    pub fn validate_proof<'info>(
        ctx: Context<'info, ValidateProof<'info>>,
        args: prover::ValidateProofArgs,
    ) -> Result<bool> {
        instructions::validate_proof(ctx, args)
    }

    pub fn close_proof<'info>(
        ctx: Context<'info, CloseProof<'info>>,
        intent_hash: Bytes32,
    ) -> Result<()> {
        instructions::close_proof(ctx, intent_hash)
    }
}
