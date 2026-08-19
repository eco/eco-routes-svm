use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;
use portal::types::{Reward, Route};

/// Seed for the program's single `flash_vault` PDA.
pub const FLASH_VAULT_SEED: &[u8] = b"flash_vault";

/// Seed for per-prover `prove_authority` PDAs.
pub const PROVE_AUTHORITY_SEED: &[u8] = b"prove_authority";

/// Seed for per-intent `FlashFulfillIntentAccount` buffer PDAs.
pub const FLASH_FULFILL_INTENT_SEED: &[u8] = b"flash_fulfill_intent";

/// Derives the program's `flash_vault` PDA (holds rewards during fulfillment).
pub fn flash_vault_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[FLASH_VAULT_SEED], &crate::ID)
}

/// Prove-only authority the program signs into the caller-chosen prover during
/// `flash_fulfill`'s prove leg, scoped to that prover's program ID. Kept distinct
/// from `flash_vault` on purpose: the fund-holding vault must never double as a
/// prover credential. Keep both the prover scoping and this separation — they are
/// a security boundary.
pub fn prove_authority_pda(prover: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[PROVE_AUTHORITY_SEED, prover.as_ref()], &crate::ID)
}

/// Stored `(route, reward)` pair that lets `flash_fulfill` be invoked by
/// intent hash alone, avoiding re-sending the full payload.
///
/// **Open consumption**: the PDA is seeded by `(writer, intent_hash)` to
/// prevent squatting on *write* (only the writer can extend or close their
/// buffer), but `flash_fulfill` imposes no signer check on the caller — any
/// third party may consume a committed buffer and direct the spread to their
/// own claimant. The writer receives only the buffer's rent refund. Writers
/// who want to capture the spread should bundle the final
/// `append_flash_fulfill_intent_chunk` with their own `flash_fulfill` in a
/// single transaction where the combined account list fits in 1232 bytes.
#[account]
pub struct FlashFulfillIntentAccount {
    /// Route committed by the buffered intent.
    pub route: Route,
    /// Reward committed by the buffered intent.
    pub reward: Reward,
}

impl FlashFulfillIntentAccount {
    /// Derives the buffer PDA for a given `(writer, intent_hash)` pair. Binding
    /// to `writer` prevents squatting: only the writer can derive (and thus
    /// extend or close) their buffer.
    pub fn pda(writer: &Pubkey, intent_hash: &Bytes32) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                FLASH_FULFILL_INTENT_SEED,
                writer.as_ref(),
                intent_hash.as_ref(),
            ],
            &crate::ID,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_vault_pda_deterministic() {
        goldie::assert_json!(flash_vault_pda());
    }

    #[test]
    fn prove_authority_pda_deterministic() {
        goldie::assert_json!(prove_authority_pda(&Pubkey::new_from_array([9u8; 32])));
    }

    #[test]
    fn flash_fulfill_intent_pda_deterministic() {
        let writer = Pubkey::new_from_array([7u8; 32]);
        let intent_hash = Bytes32::from([42u8; 32]);
        goldie::assert_json!(FlashFulfillIntentAccount::pda(&writer, &intent_hash));
    }

    #[test]
    fn flash_fulfill_intent_pda_varies_by_writer() {
        let intent_hash = Bytes32::from([42u8; 32]);
        let writer_a = Pubkey::new_from_array([1u8; 32]);
        let writer_b = Pubkey::new_from_array([2u8; 32]);

        let (pda_a, _) = FlashFulfillIntentAccount::pda(&writer_a, &intent_hash);
        let (pda_b, _) = FlashFulfillIntentAccount::pda(&writer_b, &intent_hash);

        assert_ne!(pda_a, pda_b);
    }
}
