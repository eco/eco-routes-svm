use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

declare_id!("Cg6hCaRjtPJNb7PPjGi9BMLmyoLCNatbVifaQffcQhLE");

pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod aggregator_prover {
    use super::*;

    pub fn init<'info>(ctx: Context<'info, Init<'info>>) -> Result<()> {
        instructions::init(ctx)
    }

    pub fn aggregate<'info>(
        ctx: Context<'info, Aggregate<'info>>,
        intent_hash: Bytes32,
    ) -> Result<()> {
        instructions::aggregate(ctx, intent_hash)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
