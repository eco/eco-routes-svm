use anchor_lang::prelude::*;

mod close_proof;
mod prove;

pub use close_proof::*;
pub use prove::*;

#[error_code]
pub enum LocalProverError {
    InvalidPortalDispatcher,
    InvalidSourceChain,
    InvalidPortalProofCloser,
}
