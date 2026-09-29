use anchor_lang::prelude::*;
use eco_svm_std::prover::Proof;
use eco_svm_std::Bytes32;

use crate::instructions::PolymerProverError;
use crate::state::ProofAccount;

#[derive(Accounts)]
#[instruction(intent_hash: Bytes32)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&intent_hash).0 @ PolymerProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut, address = Proof::pda(&intent_hash, &crate::ID).0 @ PolymerProverError::InvalidProof)]
    pub proof: Account<'info, ProofAccount>,
    #[account(mut)]
    pub payer: Signer<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>, _intent_hash: Bytes32) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.payer.to_account_info())
}
