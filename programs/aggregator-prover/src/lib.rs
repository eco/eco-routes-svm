use anchor_lang::prelude::*;

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

    pub fn get_proof<'info>(
        ctx: Context<'info, GetProof<'info>>,
        args: prover::GetProofArgs,
    ) -> Result<Option<prover::Proof>> {
        instructions::get_proof(ctx, args)
    }

    pub fn close_proof<'info>(
        ctx: Context<'info, CloseProof<'info>>,
        args: prover::CloseProofArgs,
    ) -> Result<()> {
        instructions::close_proof(ctx, args)
    }
}
