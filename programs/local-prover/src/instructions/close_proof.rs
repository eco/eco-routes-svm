use anchor_lang::prelude::*;
use eco_svm_std::prover::{CloseProofArgs, Proof};

use crate::instructions::LocalProverError;
use crate::state::ProofAccount;

#[derive(Accounts)]
#[instruction(args: CloseProofArgs)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&args.intent_hash).0 @ LocalProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut, address = Proof::pda(&args.intent_hash, &crate::ID).0 @ LocalProverError::InvalidProof)]
    pub proof: Account<'info, ProofAccount>,
    #[account(mut)]
    pub payer: Signer<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>, _args: CloseProofArgs) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.payer.to_account_info())
}
