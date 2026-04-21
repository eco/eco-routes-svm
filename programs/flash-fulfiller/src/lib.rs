use anchor_lang::prelude::*;

declare_id!("2d8yK5bxGuoZssTgEa4Lj9Z5AyhmVySVDQ1JadaTLeaK");

pub mod cpi;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

#[program]
pub mod flash_fulfiller {
    use super::*;

    pub fn set_flash_fulfill_intent(
        ctx: Context<SetFlashFulfillIntent>,
        args: SetFlashFulfillIntentArgs,
    ) -> Result<()> {
        instructions::set_flash_fulfill_intent(ctx, args)
    }

    pub fn flash_fulfill<'info>(
        ctx: Context<'_, '_, '_, 'info, FlashFulfill<'info>>,
        args: FlashFulfillArgs,
    ) -> Result<()> {
        instructions::flash_fulfill(ctx, args)
    }
}
