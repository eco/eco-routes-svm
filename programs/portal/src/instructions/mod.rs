use anchor_lang::prelude::*;

pub mod publish;

pub use publish::*;

#[error_code]
pub enum PortalError {
    InvalidIntentCreator,
    DuplicateTokens,
    EmptyCalls,
    InvalidIntentDeadline,
    InvalidTokenAmount,
}
