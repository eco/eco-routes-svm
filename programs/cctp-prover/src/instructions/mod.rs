use anchor_lang::prelude::*;

mod get_proof;
mod init;
mod prove;

pub use get_proof::*;
pub use init::*;
pub use prove::*;

/// The full list up front so error codes stay stable as instructions land.
#[error_code]
pub enum CctpProverError {
    InvalidPortalDispatcher,
    InvalidPortalProofCloser,
    InvalidAuthority,
    InvalidConfig,
    InvalidPdaPayer,
    InvalidPeerSet,
    InvalidData,
    InvalidDomainId,
    TooManyIntents,
    InvalidSenderAuthority,
    InvalidTransmitter,
    InvalidTransmitterAuthority,
    UnknownDomain,
    InvalidSender,
    ChainIdMismatch,
    InvalidProof,
    IntentAlreadyProven,
}
