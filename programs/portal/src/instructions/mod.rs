use anchor_lang::prelude::*;

mod fulfill;
mod fund;
mod fund_context;
mod publish;
mod refund;
mod withdraw;

pub use fulfill::*;
pub use fund::*;
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
    InvalidFulfillTarget,
    InvalidFulfillMarker,
}
