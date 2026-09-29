use anchor_lang::prelude::*;

mod close_proof;
mod init;
mod prove;
mod validate;
mod validate_proof;

pub use close_proof::*;
pub use init::*;
pub use prove::*;
pub use validate::*;
pub use validate_proof::*;

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
