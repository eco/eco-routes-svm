use anchor_lang::prelude::*;

mod prove;

pub use prove::*;

#[error_code]
pub enum HyperProverError {
    InvalidPortalDispatcher,
    InvalidDispatcher,
    InvalidData,
    InvalidMailbox,
    InvalidChainId,
}
