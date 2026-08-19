use anchor_lang::prelude::*;

mod close_fulfill_marker;
mod fulfill;
mod fund;
mod fund_context;
mod prove;
mod publish;
mod refund;
mod withdraw;

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
}
