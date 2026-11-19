use anchor_lang::prelude::*;
use eco_svm_std::claimant;
use eco_svm_std::prover::{CloseProofArgs, Proof};

use crate::instructions::CctpProverError;
use crate::state::ProofAccount;

/// Cleanup tail `[proof, rent_recipient]`. The relayer paid the rent in
/// `handle_receive_*`; it goes to the proof's claimant, who need not sign. A
/// cancelled or zero claimant is not payable (nobody controls the sentinel,
/// and the zero key is the executable System Program), so its recipient must
/// sign; Portal and the aggregator forward the tail's signer flags. A closer
/// could still name a PDA that signs through duplicate-account flag sharing
/// (e.g. Portal's `proof_closer`), which only forfeits the rent.
///
/// A payable claimant that cannot be credited (an executable program ID, a
/// sysvar or reserved key, or the `Proof` PDA itself) makes this fail
/// permanently and its rent stays locked. Settlement is unaffected: Portal's
/// `close_proof` is standalone and the aggregator closes member subsets.
/// Takes no dependency beyond its own accounts, so finalizing the program
/// cannot break it.
#[derive(Accounts)]
#[instruction(args: CloseProofArgs)]
pub struct CloseProof<'info> {
    #[account(address = portal::state::proof_closer_pda(&args.intent_hash).0 @ CctpProverError::InvalidPortalProofCloser)]
    pub portal_proof_closer: Signer<'info>,
    #[account(mut, address = Proof::pda(&args.intent_hash, &crate::ID).0 @ CctpProverError::InvalidProof)]
    pub proof: Account<'info, ProofAccount>,
    /// CHECK: the claimant, or a signer when the claimant is cancelled or zero
    #[account(
        mut,
        constraint = is_rent_recipient(&proof.0, &rent_recipient) @ CctpProverError::InvalidRentRecipient,
    )]
    pub rent_recipient: UncheckedAccount<'info>,
}

fn is_rent_recipient(proof: &Proof, recipient: &AccountInfo) -> bool {
    if claimant::is_payable(&proof.claimant) {
        recipient.key() == proof.claimant
    } else {
        recipient.is_signer
    }
}

pub fn close_proof(ctx: Context<CloseProof>, _args: CloseProofArgs) -> Result<()> {
    ctx.accounts
        .proof
        .close(ctx.accounts.rent_recipient.to_account_info())
}
