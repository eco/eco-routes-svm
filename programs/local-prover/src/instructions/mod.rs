use anchor_lang::prelude::*;

mod close_proof;
mod prove;
mod validate_proof;

pub use close_proof::*;
pub use prove::*;
pub use validate_proof::*;

#[error_code]
pub enum LocalProverError {
    InvalidCaller,
    InvalidDomainId,
    InvalidDestination,
    InvalidPortalProofCloser,
    InvalidProof,
    IntentAlreadyProven,
}
