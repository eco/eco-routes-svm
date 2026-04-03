use anchor_lang::prelude::*;

declare_id!("EcopAhGGe8ngNYcGDLeXixLrecw9H58vxQhoJkcHw678");

pub mod igp;
pub mod instructions;

use instructions::*;

#[program]
pub mod proof_helper {
    use super::*;

    pub fn pay_for_gas(ctx: Context<PayForGas>, args: PayForGasArgs) -> Result<()> {
        instructions::pay_for_gas(ctx, args)
    }
}
