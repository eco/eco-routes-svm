use anchor_lang::prelude::*;

// Task 1 scaffolds `init` only; later tasks add these modules as their files appear.
// mod close_proof;
mod init;
mod prove;
mod validate;

// pub use close_proof::*;
pub use init::*;
pub use prove::*;
pub use validate::*;

#[error_code]
pub enum PolymerProverError {
    InvalidPortalDispatcher,
    InvalidPortalProofCloser,
    InvalidConfig,
    TooManyWhitelistedEmitters,
    InvalidPolymerProver,
    InvalidCacheAccount,
    InvalidResultAccount,
    InvalidInternalAccount,
    PolymerProofInvalid,
    InvalidEmittingContract,
    InvalidTopicsLength,
    InvalidEventSignature,
    InvalidSourceChain,
    InvalidDestinationChain,
    InvalidEventData,
    InvalidProof,
    IntentAlreadyProven,
    InvalidDestination,
    EmptyProofData,
    TooManyIntents,
}
