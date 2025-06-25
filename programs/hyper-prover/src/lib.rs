use anchor_lang::prelude::*;
use eco_svm_std::prover;

declare_id!("B4pMQaAGPZ7Mza9XnDxJfXZ1cUa4aa67zrNkv8zYAjx4");

pub mod hyperlane;
pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod hyper_prover {
    use super::*;

    pub fn prove(ctx: Context<Prove>, args: prover::ProveArgs) -> Result<()> {
        prove_intent(ctx, args)
    }
}
