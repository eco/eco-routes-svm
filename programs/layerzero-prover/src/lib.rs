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

    pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
        instructions::init(ctx, args)
    }

    pub fn init_path(ctx: Context<InitPath>, eid: u32) -> Result<()> {
        instructions::init_path(ctx, eid)
    }

    pub fn set_path_config(
        ctx: Context<SetPathConfig>,
        eid: u32,
        config: PathConfig,
    ) -> Result<()> {
        instructions::set_path_config(ctx, eid, config)
    }

    pub fn set_alt(ctx: Context<SetAlt>) -> Result<()> {
        instructions::set_alt(ctx)
    }

    pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
        instructions::close_proof(ctx)
    }
}
