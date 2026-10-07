use anchor_lang::prelude::*;

declare_id!("EcoZ3pDi8PJf9ohgg6HJdgPviSqQCnbgD14KCnE4rEZm");

pub mod constants;
pub mod instructions;
pub mod layerzero;
pub mod state;

use instructions::*;

#[program]
pub mod layerzero_prover {
    use super::*;

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
