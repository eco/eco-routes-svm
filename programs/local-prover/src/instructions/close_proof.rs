use anchor_lang::prelude::*;

use crate::instructions::LocalProverError;
use crate::state::ProofAccount;

#[derive(Accounts)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda().0 @ LocalProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut)]
    pub proof: Account<'info, ProofAccount>,
    #[account(mut)]
    pub payer: Signer<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.payer.to_account_info())
}
