use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;

#[derive(Accounts)]
pub struct Ism {}

pub fn ism(_ctx: Context<Ism>) -> Result<()> {
    set_return_data(None::<Pubkey>.try_to_vec()?.as_slice());
    Ok(())
}
