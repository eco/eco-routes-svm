use anchor_lang::prelude::*;

mod close_proof;
mod get_proof;
mod init;
mod member;

pub use close_proof::*;
pub use get_proof::*;
pub use init::*;
use member::member_accounts;
pub use member::MemberQuery;

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
