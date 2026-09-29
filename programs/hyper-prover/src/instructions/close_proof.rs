use anchor_lang::prelude::*;
use eco_svm_std::prover::Proof;
use eco_svm_std::Bytes32;

use crate::instructions::HyperProverError;
use crate::state::{pda_payer_pda, ProofAccount};

#[derive(Accounts)]
#[instruction(intent_hash: Bytes32)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&intent_hash).0 @ HyperProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut, address = Proof::pda(&intent_hash, &crate::ID).0 @ HyperProverError::InvalidProof)]
    pub proof: Account<'info, ProofAccount>,
    /// CHECK: address is validated
    #[account(mut, address = pda_payer_pda().0 @ HyperProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>, _intent_hash: Bytes32) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.pda_payer.to_account_info())
}
