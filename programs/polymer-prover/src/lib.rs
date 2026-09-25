use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("EcotL2wbUqtRAjnf1p6aa842dM4fc8ZX6JhygibtBreo");

pub mod event;
pub mod instructions;
pub mod polymer;
pub mod state;

use instructions::*;

#[program]
pub mod polymer_prover {
    use super::*;

    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }

    pub fn validate<'info>(ctx: Context<'info, Validate<'info>>) -> Result<()> {
        instructions::validate(ctx)
    }

    pub fn prove(ctx: Context<Prove>, args: prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
