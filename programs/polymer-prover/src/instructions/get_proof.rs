use anchor_lang::prelude::*;
use eco_svm_std::prover::{GetProofArgs, Proof};

#[derive(Accounts)]
pub struct GetProof<'info> {
    /// CHECK: canonical address and proof contents are checked by the shared validator.
    pub proof: UncheckedAccount<'info>,
}

pub fn get_proof(ctx: Context<GetProof>, args: GetProofArgs) -> Result<Option<Proof>> {
    let GetProofArgs { intent_hash, .. } = args;

    Proof::get(&ctx.accounts.proof, &crate::ID, &intent_hash)
}
