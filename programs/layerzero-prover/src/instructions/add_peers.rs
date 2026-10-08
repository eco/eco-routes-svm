use anchor_lang::prelude::*;

use crate::instructions::LayerZeroProverError;
use crate::state::{Peer, Store};

/// Appends peers beyond the ~18 that fit in `init`'s transaction. Like the
/// rest of setup it is gated on the upgrade authority, so finalizing the
/// program fixes the peer set for good.
#[derive(Accounts)]
pub struct AddPeers<'info> {
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(mut, address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
}

pub fn add_peers(ctx: Context<AddPeers>, peers: Vec<Peer>) -> Result<()> {
    ctx.accounts.store.add_peers(peers)
}
