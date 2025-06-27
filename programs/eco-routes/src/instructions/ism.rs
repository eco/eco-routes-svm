use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct Ism {}

pub fn ism(_ctx: Context<Ism>) -> Result<()> {
    Ok(())
}
