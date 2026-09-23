use anchor_lang::prelude::*;

mod close_proof;
mod init;
mod prove;

pub use close_proof::*;
pub use init::*;
pub use prove::*;

#[error_code]
pub enum AggregatorProverError {
    InvalidConfig,
    InvalidAuthority,
    InvalidMemberSet,
    InvalidMember,
    DuplicateMember,
    InvalidDomainId,
    InvalidData,
    InvalidProof,
    InvalidIntentHash,
    NoMatchingProof,
    ClaimantMismatch,
    IntentAlreadyProven,
    InvalidPortalProofCloser,
}
