use anchor_lang::prelude::*;

use crate::instructions::HyperProverError;
use crate::state::{pda_payer_pda, ProofAccount};

#[derive(Accounts)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda().0 @ HyperProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut)]
    pub proof: Account<'info, ProofAccount>,
    /// CHECK: address is validated
    #[account(mut, address = pda_payer_pda().0 @ HyperProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.pda_payer.to_account_info())
}
