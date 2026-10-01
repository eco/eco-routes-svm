use std::ops::Range;

use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::{Bytes32, SerializableAccountMeta};

use crate::state::{FulfillMarker, FULFILL_MARKER_SEED};
use crate::types::{Call, Calldata, CalldataWithAccounts, Route};

mod cancel;
mod close_proof;
mod fulfill;
mod fund;
mod fund_context;
mod prove;
mod publish;
mod refund;
mod withdraw;

pub use cancel::*;
pub use close_proof::*;
pub use fulfill::*;
pub use fund::*;
pub use prove::*;
pub use publish::*;
pub use refund::*;
pub use withdraw::*;

pub fn now() -> Result<u64> {
    Ok(Clock::get()?
        .unix_timestamp
        .try_into()
        .expect("timestamp must fit in u64"))
}

/// Claims the intent's fulfill-marker PDA, the one record both `fulfill` (the
/// solver's claimant) and `cancel` (the cancellation claimant) write: whichever
/// creates it first owns the intent, and every later attempt fails with
/// `IntentAlreadyFulfilledOrCancelled` because the PDA is occupied.
pub(crate) fn claim_fulfill_marker<'info>(
    fulfill_marker: &UncheckedAccount<'info>,
    payer: &Signer<'info>,
    system_program: &Program<'info, System>,
    intent_hash: &Bytes32,
    claimant: Bytes32,
) -> Result<()> {
    let (expected_fulfill_marker, bump) = FulfillMarker::pda(intent_hash);
    require!(
        fulfill_marker.key() == expected_fulfill_marker,
        PortalError::InvalidFulfillMarker
    );
    let signer_seeds = [FULFILL_MARKER_SEED, intent_hash.as_ref(), &[bump]];

    FulfillMarker::new(claimant, bump)
        .init(fulfill_marker, payer, system_program, &[&signer_seeds])
        .map_err(|_| PortalError::IntentAlreadyFulfilledOrCancelled.into())
}

/// Rebuilds the committed route from `fulfill`'s compact form, the one
/// encoding both `fulfill` and `cancel` hash: each call's `data` is a borsh
/// `Calldata`, and its accounts are the next `account_count` entries of
/// `metas`, which become its `CalldataWithAccounts`.
///
/// `on_call` receives each call with the index range of its accounts in
/// `metas` before the call is rebuilt, so a caller that executes the calls
/// splits its accounts exactly as they are committed. Returns the route with
/// the number of metas the calls consumed, which is never more than
/// `metas.len()`; whether leftovers are allowed is the caller's decision.
pub(crate) fn canonical_route(
    mut route: Route,
    metas: &[SerializableAccountMeta],
    mut on_call: impl FnMut(&Call, &Calldata, Range<usize>) -> Result<()>,
) -> Result<(Route, usize)> {
    let mut start = 0;

    route.calls.iter_mut().try_for_each(|call| {
        let calldata = Calldata::try_from_slice(&call.data)?;
        let end = start + calldata.account_count as usize;
        require!(end <= metas.len(), PortalError::InvalidCalldata);

        on_call(call, &calldata, start..end)?;
        let accounts = metas[start..end]
            .iter()
            .map(|meta| SerializableAccountMeta {
                pubkey: meta.pubkey,
                is_signer: meta.is_signer,
                is_writable: meta.is_writable,
            })
            .collect::<Vec<_>>();
        call.data = borsh::to_vec(&CalldataWithAccounts::new(calldata, accounts)?)?;
        start = end;

        Result::Ok(())
    })?;

    Ok((route, start))
}

#[error_code]
pub enum PortalError {
    InvalidCreator,
    InvalidVault,
    InvalidAta,
    InvalidMint,
    InvalidTokenProgram,
    InsufficientFunds,
    InvalidTokenTransferAccounts,
    TokenAmountOverflow,
    RewardNotExpired,
    RouteExpired,
    InvalidProof,
    IntentFulfilledAndNotWithdrawn,
    IntentAlreadyWithdrawn,
    IntentAlreadyFulfilledOrCancelled,
    IntentNotFulfilled,
    InvalidCreatorToken,
    InvalidClaimantToken,
    InvalidWithdrawnMarker,
    InvalidExecutor,
    InvalidCalldata,
    InvalidFulfillMarker,
    InvalidPortal,
    InvalidProver,
    InvalidDispatcher,
    InvalidProofCloser,
    InvalidIntentHash,
    EmptyIntentHashes,
    // Anchor assigns error codes positionally from 6000 — append only, never insert.
    /// Retired: no instruction returns it. Kept so every later code keeps its number.
    InvalidFulfillMarkerPayer,
    RouteNotExpired,
    /// The payout destination is not the claimant's derived ATA and the claimant
    /// did not sign to redirect it.
    ClaimantSignatureRequired,
    ExecutorCorrupted,
    ExecutorAtaCorrupted,
    /// `claimant` is the reserved cancellation claimant, which only `cancel` may write.
    ReservedClaimant,
    /// Retired: negative withdrawal validation now returns IntentNotFulfilled.
    IntentCancelled,
    IntentNotCancelled,
    /// A refund path that queries no prover received prover accounts or data.
    UnexpectedProverQuery,
}
