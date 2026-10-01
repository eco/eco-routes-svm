use anchor_lang::prelude::*;

mod close_proof;
mod get_proof;
mod init;
mod member;

pub use close_proof::*;
pub use get_proof::*;
pub use init::*;
pub use member::MemberQuery;

#[error_code]
pub enum AggregatorProverError {
    InvalidConfig,
    InvalidAuthority,
    InvalidProverSet,
    InvalidProver,
    DuplicateProver,
    InvalidProof,
    InvalidPortalProofCloser,
    IncompleteProverSet,
}
