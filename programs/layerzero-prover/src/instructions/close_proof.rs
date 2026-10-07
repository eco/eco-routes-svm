use anchor_lang::prelude::*;

use crate::instructions::LayerZeroProverError;
use crate::state::{pda_payer_pda, ProofAccount};

/// Takes no dependency beyond its own accounts so it cannot fail once the
/// program is finalized: `portal::refund` CPIs it on proven cancellations.
#[derive(Accounts)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&crate::ID).0 @ LayerZeroProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut)]
    pub proof: Account<'info, ProofAccount>,
    /// CHECK: address is validated
    #[account(mut, address = pda_payer_pda().0 @ LayerZeroProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.pda_payer.to_account_info())
}
