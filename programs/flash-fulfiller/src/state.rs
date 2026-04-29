use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;
use portal::types::Reward;

/// Seed for the program's single `flash_vault` PDA.
pub const FLASH_VAULT_SEED: &[u8] = b"flash_vault";

/// Seed for per-intent `FlashFulfillIntentAccount` buffer PDAs.
pub const FLASH_FULFILL_INTENT_SEED: &[u8] = b"flash_fulfill_intent";

const MAX_REWARD_TOKENS: usize = 5;
const MAX_ROUTE_TOKENS: usize = 5;
const MAX_ROUTE_CALLS: usize = 10;
const MAX_CALL_DATA: usize = 512;

const TOKEN_AMOUNT_INIT_SPACE: usize = 32 + 8;
const CALL_INIT_SPACE: usize = 32 + 4 + MAX_CALL_DATA;

/// Max serialized size of a Route whose (tokens, calls, call data) all sit at
/// their per-field maxima. This caps `route_total_size` supplied by callers.
pub const MAX_ROUTE_INIT_SPACE: usize = 32
    + 8
    + 32
    + 8
    + 4
    + MAX_ROUTE_TOKENS * TOKEN_AMOUNT_INIT_SPACE
    + 4
    + MAX_ROUTE_CALLS * CALL_INIT_SPACE;

const REWARD_INIT_SPACE: usize = 8 + 32 + 32 + 8 + 4 + MAX_REWARD_TOKENS * TOKEN_AMOUNT_INIT_SPACE;

/// Size of all non-`route_bytes` fields in `FlashFulfillIntentAccount`.
const HEADER_SPACE: usize = 32            // writer
    + REWARD_INIT_SPACE
    + 32                                  // route_hash
    + 4                                   // route_total_size
    + 4                                   // route_bytes_written
    + 1; // finalized

/// Derives the program's `flash_vault` PDA (holds rewards during fulfillment).
pub fn flash_vault_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[FLASH_VAULT_SEED], &crate::ID)
}

/// Chunked buffer holding the bytes of a `Route` committed under an intent
/// hash. Populated across one or more `append_flash_fulfill_route_chunk`
/// calls after `init_flash_fulfill_intent`, then consumed by `flash_fulfill`.
///
/// `finalized` flips to true only after the final chunk fills the buffer AND
/// both the keccak of the bytes matches the committed `route_hash` and the
/// bytes deserialize as a valid `Route`. This guarantees `flash_fulfill`
/// never encounters a buffer it cannot consume.
#[account]
pub struct FlashFulfillIntentAccount {
    /// Signer that paid rent and is the only party allowed to append/cancel.
    pub writer: Pubkey,
    /// Reward committed at init time. `reward.deadline` also gates
    /// abandonment: once it passes, anyone may close the buffer.
    pub reward: Reward,
    /// Keccak256 of the route's Borsh encoding, committed at init.
    pub route_hash: Bytes32,
    /// Total size (in bytes) of the Borsh-encoded `Route` the buffer will hold.
    pub route_total_size: u32,
    /// Number of bytes written so far via `append_flash_fulfill_route_chunk`.
    pub route_bytes_written: u32,
    /// True once keccak + Borsh decode validation have both passed.
    pub finalized: bool,
    /// Pre-allocated Borsh encoding of the committed Route, filled chunk-by-chunk.
    pub route_bytes: Vec<u8>,
}

impl FlashFulfillIntentAccount {
    /// Size (sans 8-byte discriminator) of a buffer whose `route_bytes` will
    /// hold exactly `route_total_size` bytes. Includes the 4-byte Borsh
    /// length prefix for the `Vec<u8>`.
    pub fn account_space(route_total_size: u32) -> usize {
        HEADER_SPACE + 4 + route_total_size as usize
    }

    /// Derives the buffer PDA. Seeds bind the PDA to a specific `writer` so
    /// that an attacker holding a public `(route_hash, reward)` preimage
    /// cannot squat on the legitimate writer's PDA — they can only occupy a
    /// distinct PDA at their own rent cost.
    pub fn pda(intent_hash: &Bytes32, writer: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                FLASH_FULFILL_INTENT_SEED,
                intent_hash.as_ref(),
                writer.as_ref(),
            ],
            &crate::ID,
        )
    }
}

#[cfg(test)]
mod tests {
    use portal::types::{Call, Route, TokenAmount};

    use super::*;

    #[test]
    fn flash_vault_pda_deterministic() {
        goldie::assert_json!(flash_vault_pda());
    }

    #[test]
    fn flash_fulfill_intent_pda_deterministic() {
        let intent_hash = Bytes32::from([42u8; 32]);
        let writer = Pubkey::new_from_array([7u8; 32]);
        goldie::assert_json!(FlashFulfillIntentAccount::pda(&intent_hash, &writer));
    }

    #[test]
    fn flash_fulfill_intent_pda_varies_by_writer() {
        let intent_hash = Bytes32::from([42u8; 32]);
        let writer_a = Pubkey::new_from_array([7u8; 32]);
        let writer_b = Pubkey::new_from_array([8u8; 32]);

        assert_ne!(
            FlashFulfillIntentAccount::pda(&intent_hash, &writer_a).0,
            FlashFulfillIntentAccount::pda(&intent_hash, &writer_b).0,
        );
    }

    #[test]
    fn account_space_matches_serialized_length_for_max_route() {
        let route_total_size = MAX_ROUTE_INIT_SPACE;
        let account = FlashFulfillIntentAccount {
            writer: Pubkey::default(),
            reward: max_reward(),
            route_hash: [0u8; 32].into(),
            route_total_size: route_total_size as u32,
            route_bytes_written: route_total_size as u32,
            finalized: true,
            route_bytes: vec![0u8; route_total_size],
        };

        let serialized = account.try_to_vec().unwrap();

        assert_eq!(
            serialized.len(),
            FlashFulfillIntentAccount::account_space(route_total_size as u32),
        );
    }

    #[test]
    fn account_space_matches_serialized_length_for_small_route() {
        let route_total_size = 32;
        let account = FlashFulfillIntentAccount {
            writer: Pubkey::default(),
            reward: max_reward(),
            route_hash: [0u8; 32].into(),
            route_total_size: route_total_size as u32,
            route_bytes_written: 0,
            finalized: false,
            route_bytes: vec![0u8; route_total_size],
        };

        let serialized = account.try_to_vec().unwrap();

        assert_eq!(
            serialized.len(),
            FlashFulfillIntentAccount::account_space(route_total_size as u32),
        );
    }

    #[test]
    fn max_route_serializes_within_max_route_init_space() {
        let route = Route {
            salt: [0u8; 32].into(),
            deadline: 0,
            portal: [0u8; 32].into(),
            native_amount: 0,
            tokens: vec![
                TokenAmount {
                    token: Pubkey::default(),
                    amount: 0,
                };
                MAX_ROUTE_TOKENS
            ],
            calls: vec![
                Call {
                    target: [0u8; 32].into(),
                    data: vec![0u8; MAX_CALL_DATA],
                };
                MAX_ROUTE_CALLS
            ],
        };

        assert_eq!(route.try_to_vec().unwrap().len(), MAX_ROUTE_INIT_SPACE);
    }

    fn max_reward() -> Reward {
        Reward {
            deadline: 0,
            creator: Pubkey::default(),
            prover: Pubkey::default(),
            native_amount: 0,
            tokens: vec![
                TokenAmount {
                    token: Pubkey::default(),
                    amount: 0,
                };
                MAX_REWARD_TOKENS
            ],
        }
    }
}
