use anchor_lang::prelude::*;

mod close_proof;
mod init;
mod validate_proof;

pub use close_proof::*;
pub use init::*;
pub use validate_proof::*;

use crate::state;

#[error_code]
pub enum AggregatorProverError {
    InvalidConfig,
    InvalidAuthority,
    InvalidProverSet,
    InvalidProver,
    DuplicateProver,
    InvalidProof,
    InvalidPortalProofCloser,
}

pub(crate) fn validate_member(config: &state::Config, prover: &AccountInfo) -> Result<()> {
    require!(
        prover.executable && config.provers.contains(prover.key),
        AggregatorProverError::InvalidProver
    );

    Ok(())
}
