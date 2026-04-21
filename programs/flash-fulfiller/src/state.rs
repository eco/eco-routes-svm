use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;
use portal::types::{Reward, Route};

pub const FLASH_VAULT_SEED: &[u8] = b"flash_vault";
pub const FLASH_FULFILL_INTENT_SEED: &[u8] = b"flash_fulfill_intent";

const MAX_REWARD_TOKENS: usize = 5;
const MAX_ROUTE_TOKENS: usize = 5;
const MAX_ROUTE_CALLS: usize = 10;
const MAX_CALL_DATA: usize = 512;

const TOKEN_AMOUNT_INIT_SPACE: usize = 32 + 8;
const CALL_INIT_SPACE: usize = 32 + 4 + MAX_CALL_DATA;

const ROUTE_INIT_SPACE: usize = 32
    + 8
    + 32
    + 8
    + 4
    + MAX_ROUTE_TOKENS * TOKEN_AMOUNT_INIT_SPACE
    + 4
    + MAX_ROUTE_CALLS * CALL_INIT_SPACE;

const REWARD_INIT_SPACE: usize = 8 + 32 + 32 + 8 + 4 + MAX_REWARD_TOKENS * TOKEN_AMOUNT_INIT_SPACE;

pub fn flash_vault_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[FLASH_VAULT_SEED], &crate::ID)
}

#[account]
pub struct FlashFulfillIntentAccount {
    pub writer: Pubkey,
    pub route: Route,
    pub reward: Reward,
}

// TODO: drop this manual Space impl once Route/Reward (and their nested
// TokenAmount/Call) derive InitSpace in portal::types. We avoid adding
// InitSpace + max_len attributes to portal source purely to sidestep
// re-deploy considerations on the deployed portal program.
impl Space for FlashFulfillIntentAccount {
    const INIT_SPACE: usize = 32 + ROUTE_INIT_SPACE + REWARD_INIT_SPACE;
}

impl AccountExt for FlashFulfillIntentAccount {}

impl FlashFulfillIntentAccount {
    pub fn pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[FLASH_FULFILL_INTENT_SEED, intent_hash.as_ref()],
            &crate::ID,
        )
    }
}

#[cfg(test)]
mod tests {
    use portal::types::{Call, TokenAmount};

    use super::*;

    #[test]
    fn flash_vault_pda_deterministic() {
        goldie::assert_json!(flash_vault_pda());
    }

    #[test]
    fn flash_fulfill_intent_pda_deterministic() {
        let intent_hash = Bytes32::from([42u8; 32]);
        goldie::assert_json!(FlashFulfillIntentAccount::pda(&intent_hash));
    }

    #[test]
    fn flash_fulfill_intent_init_space_matches_max_sized_payload() {
        let max_token = TokenAmount {
            token: Pubkey::default(),
            amount: 0,
        };
        let max_call = Call {
            target: [0u8; 32].into(),
            data: vec![0u8; MAX_CALL_DATA],
        };
        let account = FlashFulfillIntentAccount {
            writer: Pubkey::default(),
            route: Route {
                salt: [0u8; 32].into(),
                deadline: 0,
                portal: [0u8; 32].into(),
                native_amount: 0,
                tokens: vec![max_token.clone(); MAX_ROUTE_TOKENS],
                calls: vec![max_call; MAX_ROUTE_CALLS],
            },
            reward: Reward {
                deadline: 0,
                creator: Pubkey::default(),
                prover: Pubkey::default(),
                native_amount: 0,
                tokens: vec![max_token; MAX_REWARD_TOKENS],
            },
        };

        let serialized = account.try_to_vec().unwrap();

        assert_eq!(serialized.len(), FlashFulfillIntentAccount::INIT_SPACE);
    }
}
