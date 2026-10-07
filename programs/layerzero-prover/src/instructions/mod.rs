use anchor_lang::prelude::*;

mod close_proof;
mod init;
mod init_path;
mod lz_receive;
mod lz_receive_types;
mod prove;
mod quote_message;
mod send_message;
mod set_alt;
mod set_path_config;

pub use close_proof::*;
pub use init::*;
pub use init_path::*;
pub use lz_receive::*;
pub use lz_receive_types::*;
pub use prove::*;
pub use quote_message::*;
pub use send_message::*;
pub use set_alt::*;
pub use set_path_config::*;

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
