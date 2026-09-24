use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;

use crate::state::{FulfillMarker, FULFILL_MARKER_SEED};

mod cancel;
mod close_fulfill_marker;
mod fulfill;
mod fund;
mod fund_context;
mod prove;
mod publish;
mod refund;
mod withdraw;

pub use cancel::*;
pub use close_fulfill_marker::*;
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

/// Writes the intent's `FulfillMarker`, the one record both `fulfill` and
/// `cancel` claim: whichever creates it first owns the intent, and every later
/// attempt — including one against its closed `FulfillTombstone` — fails with
/// `IntentAlreadyFulfilled`.
pub(crate) fn create_fulfill_marker<'info>(
    fulfill_marker: &UncheckedAccount<'info>,
    payer: &Signer<'info>,
    system_program: &Program<'info, System>,
    intent_hash: &Bytes32,
    claimant: Bytes32,
    deadline: u64,
) -> Result<()> {
    let (expected_fulfill_marker, bump) = FulfillMarker::pda(intent_hash);
    require!(
        fulfill_marker.key() == expected_fulfill_marker,
        PortalError::InvalidFulfillMarker
    );
    let signer_seeds = [FULFILL_MARKER_SEED, intent_hash.as_ref(), &[bump]];

    FulfillMarker::new(claimant, payer.key(), deadline, bump)
        .init(fulfill_marker, payer, system_program, &[&signer_seeds])
        .map_err(|_| PortalError::IntentAlreadyFulfilled.into())
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
    IntentAlreadyFulfilled,
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
    InvalidFulfillMarkerPayer,
    RouteNotExpired,
    /// The payout destination is not the claimant's derived ATA and the claimant
    /// did not sign to redirect it.
    ClaimantSignatureRequired,
    ExecutorCorrupted,
    ExecutorAtaCorrupted,
    /// `claimant` is the reserved `CANCELLED` sentinel, which only `cancel` may write.
    ReservedClaimant,
    /// The intent's proof records a cancellation, which never pays a claimant.
    IntentCancelled,
}
