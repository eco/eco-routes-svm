use anchor_lang::prelude::*;

mod fund;
mod publish;
mod refund;
mod withdraw;

pub use fund::*;
pub use publish::*;
pub use refund::*;
pub use withdraw::*;

#[error_code]
pub enum PortalError {
    InvalidCreator,
    InvalidVault,
    InvalidVaultAta,
    InvalidMint,
    InvalidTokenProgram,
    InsufficientFunds,
    InvalidTokenTransferAccounts,
    RewardAmountOverflow,
    RewardNotExpired,
    InvalidProof,
    IntentFulfilledAndNotWithdrawn,
    IntentAlreadyWithdrawn,
    IntentNotFulfilled,
    InvalidCreatorToken,
    InvalidClaimantToken,
    InvalidWithdrawnMarker,
}
