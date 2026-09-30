use anchor_lang::prelude::*;

mod close_proof;
mod get_proof;
mod prove;

pub use close_proof::*;
pub use get_proof::*;
pub use prove::*;

#[error_code]
pub enum LocalProverError {
    InvalidCaller,
    InvalidDomainId,
    InvalidDestination,
    InvalidPortalProofCloser,
    InvalidProof,
    IntentAlreadyProven,
}
