use anchor_lang::prelude::*;

use crate::types::{self, Bytes32, Reward};

const VAULT_SEED: &[u8] = b"vault";

#[account]
#[derive(InitSpace)]
pub struct Vault {
    pub bump: u8,
}

impl Vault {
    pub fn pda(route_chain: Bytes32, route_hash: Bytes32, reward: &Reward) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                VAULT_SEED,
                types::intent_hash(route_chain, route_hash, reward).as_ref(),
            ],
            &crate::ID,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TokenAmount;

    #[test]
    fn vault_pda() {
        let route_chain = [5u8; 32];
        let route_hash = [6u8; 32];
        let reward = Reward {
            deadline: 1640995200,
            creator: Pubkey::new_from_array([1u8; 32]),
            prover: [2u8; 32],
            native_amount: 1_000_000_000,
            tokens: vec![
                TokenAmount {
                    token: [3u8; 32],
                    amount: 100,
                },
                TokenAmount {
                    token: [4u8; 32],
                    amount: 200,
                },
            ],
        };

        goldie::assert_json!(Vault::pda(route_chain, route_hash, &reward));
    }
}
