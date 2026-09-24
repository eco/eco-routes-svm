use anchor_lang::prelude::*;

mod aggregate;
mod close_proof;
mod init;

pub use aggregate::*;
pub use close_proof::*;
pub use init::*;

#[error_code]
pub enum AggregatorProverError {
    InvalidConfig,
    InvalidAuthority,
    InvalidProverSet,
    InvalidProver,
    DuplicateProver,
    InvalidData,
    InvalidProof,
    InvalidIntentHash,
    NoMatchingProof,
    ClaimantMismatch,
    IntentAlreadyProven,
    InvalidPortalProofCloser,
}
