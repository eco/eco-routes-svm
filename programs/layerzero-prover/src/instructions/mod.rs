use anchor_lang::prelude::*;

mod close_proof;

pub use close_proof::*;

#[error_code]
pub enum LayerZeroProverError {
    InvalidPortalDispatcher,
    InvalidPortalProofCloser,
    InvalidAuthority,
    InvalidStore,
    InvalidLzReceiveTypes,
    InvalidPdaPayer,
    InvalidPeerSet,
    UnknownPeer,
    InvalidReceiver,
    InvalidData,
    InvalidDomainId,
    EmptyProof,
    TooManyIntents,
    UnpinnedConfig,
    AltNotSet,
    InvalidEndpoint,
    InvalidUln,
    InvalidSender,
    ChainIdMismatch,
    InvalidProof,
    IntentAlreadyProven,
    InvalidPendingSend,
    InvalidRentPayer,
    InvalidQuote,
}
