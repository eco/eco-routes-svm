use anchor_lang::prelude::*;

mod fund;
mod publish;
mod refund;

pub use fund::*;
pub use publish::*;
pub use refund::*;

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
    IntentAlreadyFulfilled,
    InvalidCreatorToken,
}
