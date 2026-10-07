use anchor_lang::prelude::*;

use crate::instructions::LayerZeroProverError;
use crate::state::Store;

pub const ADDRESS_LOOKUP_TABLE_PROGRAM_ID: Pubkey =
    pubkey!("AddressLookupTab1e1111111111111111111111111");

/// Records the lookup table returned to the executor in
/// `lz_receive_types_v2`. Freeze the table before finalizing the program.
#[derive(Accounts)]
pub struct SetAlt<'info> {
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(mut, address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: owner is validated
    #[account(owner = ADDRESS_LOOKUP_TABLE_PROGRAM_ID)]
    pub alt: UncheckedAccount<'info>,
}

pub fn set_alt(ctx: Context<SetAlt>) -> Result<()> {
    ctx.accounts.store.alt = ctx.accounts.alt.key();
    Ok(())
}
