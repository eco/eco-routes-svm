use anchor_lang::prelude::*;
use eco_svm_std::prover::{CloseProofArgs, Proof};

use crate::instructions::CctpProverError;
use crate::state::ProofAccount;

/// Cleanup tail `[proof, rent_recipient]`. The relayer paid the rent in
/// `handle_receive_*`; it goes to the proof's claimant, who need not sign. A
/// cancellation's claimant is the uncontrolled sentinel, so its rent goes to
/// a recipient that signs: Portal and the aggregator forward the tail's signer
/// flags, so only the closer can name itself. Takes no dependency beyond its
/// own accounts so it cannot fail once the program is finalized.
#[derive(Accounts)]
#[instruction(args: CloseProofArgs)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&args.intent_hash).0 @ CctpProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut, address = Proof::pda(&args.intent_hash, &crate::ID).0 @ CctpProverError::InvalidProof)]
    pub proof: Account<'info, ProofAccount>,
    /// CHECK: the claimant, or a signer for a cancellation
    #[account(
        mut,
        constraint = is_rent_recipient(&proof.0, &rent_recipient) @ CctpProverError::InvalidRentRecipient,
    )]
    pub rent_recipient: UncheckedAccount<'info>,
}

fn is_rent_recipient(proof: &Proof, recipient: &AccountInfo) -> bool {
    if proof.is_cancelled() {
        recipient.is_signer
    } else {
        recipient.key() == proof.claimant
    }
}

pub fn close_proof(ctx: Context<CloseProof>, _args: CloseProofArgs) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.rent_recipient.to_account_info())
}
