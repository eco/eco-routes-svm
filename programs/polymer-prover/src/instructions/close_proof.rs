use anchor_lang::prelude::*;

use crate::instructions::PolymerProverError;
use crate::state::ProofAccount;

#[derive(Accounts)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&crate::ID).0 @ PolymerProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut)]
    pub proof: Account<'info, ProofAccount>,
    #[account(mut)]
    pub payer: Signer<'info>,
}

/// Closes the proof to `payer`, the account paying for Portal's `withdraw`.
pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.payer.to_account_info())
}
