use anchor_lang::prelude::*;

mod fulfill;
mod fund;
mod fund_context;
mod prove;
mod publish;
mod refund;
mod withdraw;

pub use fulfill::*;
pub use fund::*;
pub use prove::*;
pub use publish::*;
pub use refund::*;
pub use withdraw::*;

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
}
