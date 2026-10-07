use anchor_lang::prelude::*;
use eco_svm_std::prover::{CloseProofArgs, Proof};

use crate::instructions::HyperProverError;
use crate::state::{pda_payer_pda, ProofAccount};

#[derive(Accounts)]
#[instruction(args: CloseProofArgs)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&args.intent_hash).0 @ HyperProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut, address = Proof::pda(&args.intent_hash, &crate::ID).0 @ HyperProverError::InvalidProof)]
    pub proof: Account<'info, ProofAccount>,
    /// CHECK: address is validated
    #[account(mut, address = pda_payer_pda().0 @ HyperProverError::InvalidPdaPayer)]
    pub pda_payer: UncheckedAccount<'info>,
}

pub fn close_proof(ctx: Context<CloseProof>, _args: CloseProofArgs) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.pda_payer.to_account_info())
}
