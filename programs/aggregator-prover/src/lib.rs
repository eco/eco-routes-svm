use anchor_lang::prelude::*;
use eco_svm_std::prover::ProveArgs;

declare_id!("Cg6hCaRjtPJNb7PPjGi9BMLmyoLCNatbVifaQffcQhLE");

pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod aggregator_prover {
    use super::*;

    pub fn init<'info>(ctx: Context<'info, Init<'info>>, members: Vec<Pubkey>) -> Result<()> {
        instructions::init(ctx, members)
    }

    pub fn prove<'info>(ctx: Context<'info, Prove<'info>>, args: ProveArgs) -> Result<()> {
        instructions::prove(ctx, args)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
