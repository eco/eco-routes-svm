use anchor_lang::prelude::*;

mod close_proof;
mod handle;
mod handle_account_metas;
mod init;
mod ism;
mod ism_account_metas;
mod prove;

pub use close_proof::*;
pub use handle::*;
pub use handle_account_metas::*;
pub use init::*;
pub use ism::*;
pub use ism_account_metas::*;
pub use prove::*;

#[error_code]
pub enum HyperProverError {
    InvalidPortalDispatcher,
    InvalidPortalProofCloser,
    InvalidDispatcher,
    InvalidData,
    InvalidMailbox,
    InvalidDomainId,
    InvalidProcessAuthority,
    InvalidConfig,
    TooManyWhitelistedSenders,
    InvalidSender,
    InvalidProof,
    IntentAlreadyProven,
    InvalidPdaPayer,
}
