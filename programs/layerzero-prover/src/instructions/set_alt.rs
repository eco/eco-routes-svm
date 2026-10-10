use anchor_lang::prelude::*;

use crate::instructions::{required_alt_addresses, LayerZeroProverError};
use crate::state::Store;

pub const ADDRESS_LOOKUP_TABLE_PROGRAM_ID: Pubkey =
    pubkey!("AddressLookupTab1e1111111111111111111111111");

/// Lookup table account layout, from `solana-address-lookup-table-interface`
/// 3.1.0 `state.rs`: bincode `ProgramState::LookupTable(LookupTableMeta)` in
/// the first `LOOKUP_TABLE_META_SIZE` bytes, then the 32-byte addresses.
/// Meta: u32 LE variant tag, `deactivation_slot` u64 LE, `last_extended_slot`
/// u64 LE, `last_extended_slot_start_index` u8, `authority: Option<Pubkey>`.
const LOOKUP_TABLE_META_SIZE: usize = 56;
/// `ProgramState::LookupTable` (0 is `Uninitialized`).
const LOOKUP_TABLE_TAG: [u8; 4] = 1u32.to_le_bytes();
const DEACTIVATION_SLOT_RANGE: std::ops::Range<usize> = 4..12;
/// `Option<Pubkey>` tag of `authority`: 0 = `None`, i.e. frozen.
const AUTHORITY_OPTION_OFFSET: usize = 21;

/// Records the lookup table returned to the executor in
/// `lz_receive_types_v2`. Once the program is finalized `set_alt` can never
/// run again, so the table must already be permanent and complete.
#[derive(Accounts)]
pub struct SetAlt<'info> {
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::LayerzeroProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ LayerZeroProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(mut, address = Store::pda().0 @ LayerZeroProverError::InvalidStore)]
    pub store: Account<'info, Store>,
    /// CHECK: owner is validated here; layout and contents in `set_alt`
    #[account(owner = ADDRESS_LOOKUP_TABLE_PROGRAM_ID @ LayerZeroProverError::InvalidLookupTable)]
    pub alt: UncheckedAccount<'info>,
}

pub fn set_alt(ctx: Context<SetAlt>) -> Result<()> {
    validate_lookup_table(
        &ctx.accounts.alt.try_borrow_data()?,
        &required_alt_addresses(&ctx.accounts.store),
    )?;
    ctx.accounts.store.alt = ctx.accounts.alt.key();

    Ok(())
}

/// The addresses stored in a lookup table account, in table order (an
/// address's position is its index for `AddressLocator::AltIndex`).
/// `InvalidLookupTable` unless `data` is an initialized table.
pub fn lookup_table_addresses(data: &[u8]) -> Result<Vec<Pubkey>> {
    require!(
        data.len() >= LOOKUP_TABLE_META_SIZE
            && (data.len() - LOOKUP_TABLE_META_SIZE).is_multiple_of(32)
            && data[..4] == LOOKUP_TABLE_TAG,
        LayerZeroProverError::InvalidLookupTable
    );

    Ok(data[LOOKUP_TABLE_META_SIZE..]
        .chunks_exact(32)
        .map(|address| Pubkey::try_from(address).expect("32-byte chunk"))
        .collect())
}

fn validate_lookup_table(data: &[u8], required: &[Pubkey]) -> Result<()> {
    let addresses = lookup_table_addresses(data)?;
    // Frozen (no authority) and never deactivated. The lookup-table program
    // lets only the authority extend, deactivate or close a table, and refuses
    // to freeze a deactivated one, so this state is permanent: the executor can
    // resolve these addresses for as long as the program lives.
    require!(
        data[AUTHORITY_OPTION_OFFSET] == 0,
        LayerZeroProverError::LookupTableNotFrozen
    );
    require!(
        data[DEACTIVATION_SLOT_RANGE] == u64::MAX.to_le_bytes(),
        LayerZeroProverError::LookupTableDeactivated
    );
    require!(
        required.iter().all(|key| addresses.contains(key)),
        LayerZeroProverError::LookupTableMissingAddress
    );

    Ok(())
}
