use anchor_lang::prelude::*;
use eco_svm_std::prover::{CloseProofArgs, Proof};

use crate::instructions::CctpProverError;
use crate::state::{pda_payer_pda, ProofAccount};

/// Cleanup tail `[proof, pda_payer]` (non-signer): rent returns to the reserve
/// that paid it in `handle_receive_*`. Takes no dependency beyond its own
/// accounts so it cannot fail once the program is finalized.
#[derive(Accounts)]
#[instruction(args: CloseProofArgs)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&args.intent_hash).0 @ CctpProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut, address = Proof::pda(&args.intent_hash, &crate::ID).0 @ CctpProverError::InvalidProof)]
    pub proof: Account<'info, ProofAccount>,
    /// CHECK: address is validated
    #[account(mut, address = pda_payer_pda().0 @ CctpProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>, _args: CloseProofArgs) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.pda_payer.to_account_info())
}
