use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;

#[derive(Accounts)]
pub struct Ism {}

/// Returns `None` as the ISM program ID, signalling to Hyperlane that this
/// recipient has no custom Interchain Security Module. The mailbox will
/// therefore fall back to its configured default ISM for message verification.
pub fn ism(_ctx: Context<Ism>) -> Result<()> {
    set_return_data(None::<Pubkey>.try_to_vec()?.as_slice());
    Ok(())
}
