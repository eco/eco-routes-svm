use anchor_lang::prelude::*;

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
        args: AggregateArgs,
    ) -> Result<()> {
        instructions::aggregate(ctx, args)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
