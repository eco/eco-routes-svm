use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

pub const VAULT_SEED: &[u8] = b"vault";

pub fn vault_pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_SEED, intent_hash.as_ref()], &crate::ID)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{self, Reward, TokenAmount};

    #[test]
    fn vault_pda_deterministic() {
        let destination_chain = [5u8; 32].into();
        let route_hash = [6u8; 32].into();
        let reward = Reward {
            deadline: 1640995200,
            creator: Pubkey::new_from_array([1u8; 32]),
            prover: Pubkey::new_from_array([2u8; 32]),
            native_amount: 1_000_000_000,
            tokens: vec![
                TokenAmount {
                    token: Pubkey::new_from_array([3u8; 32]),
                    amount: 100,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([4u8; 32]),
                    amount: 200,
                },
            ],
        };

        goldie::assert_json!(vault_pda(&types::intent_hash(
            &destination_chain,
            &route_hash,
            &reward
        )));
    }
}
