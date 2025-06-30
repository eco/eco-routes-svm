use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;

use crate::hyperlane;

#[derive(Accounts)]
pub struct Ism {}

pub fn ism(_ctx: Context<Ism>) -> Result<()> {
    set_return_data(
        Some(hyperlane::MULTISIG_ISM_MESSAGE_ID)
            .try_to_vec()?
            .as_slice(),
    );

    Ok(())
}
