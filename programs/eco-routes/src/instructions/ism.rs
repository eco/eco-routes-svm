use anchor_lang::{prelude::*, solana_program::program::set_return_data};

use crate::hyperlane;

#[derive(Accounts)]
pub struct Ism {}

pub fn ism(_ctx: Context<Ism>) -> Result<()> {
    set_return_data(Some(hyperlane::MULTISIG_ISM_ID).try_to_vec()?.as_slice());
    Ok(())
}
