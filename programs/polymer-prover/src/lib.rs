use anchor_lang::prelude::*;

declare_id!("EcotL2wbUqtRAjnf1p6aa842dM4fc8ZX6JhygibtBreo");

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
}
