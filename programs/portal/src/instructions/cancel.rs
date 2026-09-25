use anchor_lang::prelude::*;
use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};

use crate::events::IntentCancelled;
use crate::instructions::{create_fulfill_marker, now, PortalError};
use crate::types::{self, Route};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CancelArgs {
    pub intent_hash: Bytes32,
    pub route: Route,
    pub reward_hash: Bytes32,
}

/// Permanently closes an unfulfilled intent on its destination.
///
/// Writes the `CANCELLED` sentinel into the intent's `FulfillMarker` PDA, the
/// same record `fulfill` writes, so the two are mutually exclusive by
/// construction: whichever lands first owns the PDA and the other fails to
/// create it. Time separates them too — `fulfill` needs
/// `route.deadline >= now`, `cancel` needs `route.deadline < now` — so they
/// never race.
///
/// Permissionless: the caller chooses only *when* after the deadline, never
/// the outcome. `prove` then carries the sentinel to the source unchanged,
/// where it enables `refund` before `reward.deadline`.
///
/// `route` is the canonical route the source committed to; it is hashed as
/// passed (no calls are executed).
#[derive(Accounts)]
#[instruction(args: CancelArgs)]
pub struct Cancel<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub fulfill_marker: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn cancel_intent(ctx: Context<Cancel>, args: CancelArgs) -> Result<()> {
    let CancelArgs {
        intent_hash: expected_intent_hash,
        route,
        reward_hash,
    } = args;

    require!(route.portal == crate::ID, PortalError::InvalidPortal);
    require!(route.deadline < now()?, PortalError::RouteNotExpired);

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    require!(
        intent_hash == expected_intent_hash,
        PortalError::InvalidIntentHash
    );
    create_fulfill_marker(
        &ctx.accounts.fulfill_marker,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &intent_hash,
        CANCELLED,
        route.deadline,
    )?;

    emit!(IntentCancelled::new(intent_hash));

    Ok(())
}
