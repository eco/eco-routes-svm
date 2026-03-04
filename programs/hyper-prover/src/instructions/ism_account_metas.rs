use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;

#[derive(Accounts)]
pub struct IsmAccountMetas<'info> {
    /// CHECK: Simulation only
    #[account(
        seeds = [
            b"hyperlane_message_recipient",
            b"-",
            b"interchain_security_module",
            b"-",
            b"account_metas"
        ],
        bump
    )]
    pub ism_account_metas: AccountInfo<'info>,
}

pub fn ism_account_metas(_ctx: Context<IsmAccountMetas>) -> Result<()> {
    set_return_data(&[0, 0, 0, 0]); // borsh-serialized empty vec
    Ok(())
}
