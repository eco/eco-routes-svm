use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::events::FulfillMarkerClosed;
use crate::instructions::{now, PortalError};
use crate::state::{FulfillMarker, FulfillTombstone};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CloseFulfillMarkerArgs {
    pub intent_hash: Bytes32,
}

/// Reclaims most of a [`FulfillMarker`]'s rent to the payer that funded it by
/// shrinking the marker in place to a [`FulfillTombstone`].
///
/// # Why a tombstone, not a close
///
/// The PDA must stay occupied. `cancel` treats an empty marker PDA as "never
/// fulfilled", so deleting it would let anyone cancel an intent that was in
/// fact fulfilled — and if that cancellation's proof reached the source before
/// the solver's, the creator would be refunded for a delivered route. The
/// tombstone keeps `fulfill` and `cancel` failing and keeps the claimant, so
/// the intent also stays provable after the close.
///
/// Solana charges rent on a 128-byte account overhead, so shrinking 81 bytes
/// to 40 returns only about a fifth of the marker's rent.
///
/// # Deadline gate
///
/// `route.deadline` (stored in the marker because `fulfill` sees no reward)
/// must have passed: until then the marker is also the double-fulfill guard.
///
/// # The payer must be solver-controlled
///
/// `payer` is the sole authority able to close the marker and the address the
/// rent returns to, so a sponsored or ephemeral fee-payer would strand it.
#[derive(Accounts)]
#[instruction(args: CloseFulfillMarkerArgs)]
pub struct CloseFulfillMarker<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address, owner, discriminator and payer are validated in
    /// `close_fulfill_marker`. Not an `Account<FulfillMarker>`: that wrapper
    /// re-serializes the marker on exit and would overwrite the tombstone.
    #[account(mut)]
    pub fulfill_marker: UncheckedAccount<'info>,
}

pub fn close_fulfill_marker(
    ctx: Context<CloseFulfillMarker>,
    args: CloseFulfillMarkerArgs,
) -> Result<()> {
    let CloseFulfillMarkerArgs { intent_hash } = args;
    let fulfill_marker = &ctx.accounts.fulfill_marker;
    let payer = &ctx.accounts.payer;

    require!(
        fulfill_marker.key() == FulfillMarker::pda(&intent_hash).0,
        PortalError::InvalidFulfillMarker
    );
    require!(
        fulfill_marker.owner == &crate::ID,
        PortalError::InvalidFulfillMarker
    );
    let FulfillMarker {
        claimant,
        payer: marker_payer,
        deadline,
        ..
    } = FulfillMarker::try_deserialize(&mut &fulfill_marker.try_borrow_data()?[..])
        .map_err(|_| Error::from(PortalError::InvalidFulfillMarker))?;

    require!(
        marker_payer == payer.key(),
        PortalError::InvalidFulfillMarkerPayer
    );
    require!(deadline < now()?, PortalError::RouteNotExpired);

    let tombstone_len = 8 + FulfillTombstone::INIT_SPACE;
    fulfill_marker.resize(tombstone_len)?;
    FulfillTombstone::new(claimant)
        .try_serialize(&mut &mut fulfill_marker.try_borrow_mut_data()?[..])?;

    let refunded = fulfill_marker
        .get_lamports()
        .checked_sub(Rent::get()?.minimum_balance(tombstone_len))
        .ok_or(ProgramError::ArithmeticOverflow)?;
    fulfill_marker.sub_lamports(refunded)?;
    payer.add_lamports(refunded)?;

    emit!(FulfillMarkerClosed::new(
        intent_hash,
        payer.key(),
        claimant,
        refunded,
    ));

    Ok(())
}
