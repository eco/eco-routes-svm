use anchor_lang::prelude::*;
use eco_svm_std::prover::{Proof, ValidateProofArgs};

#[derive(Accounts)]
pub struct ValidateProof<'info> {
    /// CHECK: canonical address and proof contents are checked by the shared validator.
    pub proof: UncheckedAccount<'info>,
}

pub fn validate_proof(ctx: Context<ValidateProof>, args: ValidateProofArgs) -> Result<bool> {
    Proof::validate(&ctx.accounts.proof, &crate::ID, args)
}
