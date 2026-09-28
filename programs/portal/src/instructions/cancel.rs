use anchor_lang::prelude::*;
use eco_svm_std::{Bytes32, SerializableAccountMeta, CANCELLED, CHAIN_ID};

use crate::events::IntentCancelled;
use crate::instructions::{canonical_route, claim_fulfill_marker, now, PortalError};
use crate::state::{fulfillment_claimant, FulfillMarker};
use crate::types::{self, Route};

/// `CancelArgs::account_flags` bit marking a call account as a signer.
pub const ACCOUNT_FLAG_SIGNER: u8 = 1 << 0;
/// `CancelArgs::account_flags` bit marking a call account as writable.
pub const ACCOUNT_FLAG_WRITABLE: u8 = 1 << 1;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CancelArgs {
    pub intent_hash: Bytes32,
    /// The route in `fulfill`'s compact form: each call's `data` is a borsh
    /// `Calldata`, and its accounts are the next `account_count` remaining
    /// accounts.
    pub route: Route,
    pub reward_hash: Bytes32,
    /// One entry per remaining account, in order: the `is_signer`
    /// (`ACCOUNT_FLAG_SIGNER`) and `is_writable` (`ACCOUNT_FLAG_WRITABLE`) flags
    /// the source committed to for that call account.
    pub account_flags: Vec<u8>,
}

/// Permanently closes an unfulfilled intent on its destination.
///
/// Writes a `FulfillMarker` holding the `CANCELLED` sentinel at the intent's
/// fulfill-marker PDA, the same PDA `fulfill` claims, so the two are
/// mutually exclusive by construction: whichever lands first owns the PDA and
/// the other fails to create it. Time separates them too — `fulfill` needs
/// `route.deadline >= now`, `cancel` needs `route.deadline < now` — so they
/// never race.
///
/// The marker is never closed, so a cancellation stays permanent and a
/// cancelled intent can never be fulfilled, and vice versa. Cancelling an
/// already-cancelled intent is a no-op, so a front-run `cancel` never fails a
/// `cancel` + `prove` transaction.
///
/// Permissionless: the caller chooses only *when* after the deadline, never
/// the outcome. `prove` then carries the sentinel to the source unchanged,
/// where it enables `refund` before `reward.deadline`.
///
/// The canonical route is rebuilt the way `fulfill` rebuilds it: call account
/// keys come from the remaining accounts, deduplicated by the transaction. The
/// signer and writable flags come from `account_flags` instead of the account
/// infos, so the call accounts are passed read-only and unsigned. Those flags
/// cost one byte per call-account slot, repeats included, which `fulfill`
/// does not pay, so a route near the transaction size limit can fit `fulfill`
/// but not `cancel`; it then refunds from `reward.deadline` instead. No calls are
/// executed.
#[derive(Accounts)]
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
        account_flags,
    } = args;

    require!(route.portal == crate::ID, PortalError::InvalidPortal);
    require!(route.deadline < now()?, PortalError::RouteNotExpired);

    require!(
        ctx.remaining_accounts.len() == account_flags.len(),
        PortalError::InvalidCalldata
    );
    let metas = ctx
        .remaining_accounts
        .iter()
        .zip(account_flags)
        .map(|(account, flags)| account_meta(account.key(), flags))
        .collect::<Result<Vec<_>>>()?;
    let (route, consumed) = canonical_route(route, &metas, |_, _, _| Ok(()))?;
    // Every account and flag must belong to a call, so a route has one
    // accepted encoding.
    require!(consumed == metas.len(), PortalError::InvalidCalldata);
    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    require!(
        intent_hash == expected_intent_hash,
        PortalError::InvalidIntentHash
    );
    if is_cancelled(&ctx.accounts.fulfill_marker, &intent_hash)? {
        return Ok(());
    }
    claim_fulfill_marker(
        &ctx.accounts.fulfill_marker,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &intent_hash,
        CANCELLED,
    )?;

    emit!(IntentCancelled::new(intent_hash));

    Ok(())
}

/// True when the intent's fulfill marker already holds `CANCELLED`. A marker
/// holding a solver's claimant reads false, so `claim_fulfill_marker` then
/// fails with `IntentAlreadyFulfilledOrCancelled`.
fn is_cancelled(fulfill_marker: &UncheckedAccount, intent_hash: &Bytes32) -> Result<bool> {
    if fulfill_marker.owner != &crate::ID {
        return Ok(false);
    }
    require!(
        fulfill_marker.key() == FulfillMarker::pda(intent_hash).0,
        PortalError::InvalidFulfillMarker
    );

    Ok(fulfillment_claimant(fulfill_marker)? == CANCELLED)
}

/// A flag may carry only the two defined bits, so a route has one accepted
/// encoding.
fn account_meta(pubkey: Pubkey, flags: u8) -> Result<SerializableAccountMeta> {
    require!(
        flags & !(ACCOUNT_FLAG_SIGNER | ACCOUNT_FLAG_WRITABLE) == 0,
        PortalError::InvalidCalldata
    );

    Ok(SerializableAccountMeta {
        pubkey,
        is_signer: flags & ACCOUNT_FLAG_SIGNER != 0,
        is_writable: flags & ACCOUNT_FLAG_WRITABLE != 0,
    })
}
