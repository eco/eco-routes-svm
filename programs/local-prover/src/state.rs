use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;

#[account]
#[derive(InitSpace)]
pub struct ProofAccount(pub eco_svm_std::prover::Proof);

impl AccountExt for ProofAccount {}

impl From<eco_svm_std::prover::Proof> for ProofAccount {
    fn from(proof: eco_svm_std::prover::Proof) -> Self {
        Self(proof)
    }
}

#[cfg(test)]
mod tests {
    /// The two caller authorities `prove` accepts. Both are scoped to this
    /// program's ID, so a seed change here silently locks out every honest
    /// caller — pin the resulting addresses, not just the derivation.
    #[test]
    fn accepted_prove_caller_authorities_deterministic() {
        goldie::assert_json!((
            portal::state::dispatcher_pda(&crate::ID),
            flash_fulfiller::state::prove_authority_pda(&crate::ID),
        ));
    }

    /// `flash_vault` was the pre-scoping prove credential, and it is also the
    /// fund-holding identity flash-fulfiller signs into a caller-chosen program
    /// at the fulfill leg. It must never be an accepted caller again: that
    /// combination let a program chosen by the caller mint proofs for intents it
    /// never fulfilled, since `prove` performs no fulfillment check of its own.
    #[test]
    fn flash_vault_is_not_an_accepted_prove_caller() {
        let flash_vault = flash_fulfiller::state::flash_vault_pda().0;

        assert_ne!(flash_vault, portal::state::dispatcher_pda(&crate::ID).0);
        assert_ne!(
            flash_vault,
            flash_fulfiller::state::prove_authority_pda(&crate::ID).0
        );
    }
}
