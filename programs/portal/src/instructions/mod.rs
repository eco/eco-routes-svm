use anchor_lang::prelude::*;

pub mod fund;
pub mod publish;

pub use fund::*;
pub use publish::*;

#[error_code]
pub enum PortalError {
    InvalidIntentCreator,
    DuplicateTokens,
    EmptyCalls,
    InvalidIntentDeadline,
    InvalidTokenAmount,
    InvalidVault,
    InvalidVaultAta,
    InvalidMint,
    InvalidTokenProgram,
    InsufficientFunds,
    InvalidTokenTransferAccounts,
}
