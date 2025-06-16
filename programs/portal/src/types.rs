use std::collections::HashSet;

use anchor_lang::prelude::*;
use tiny_keccak::{Hasher, Keccak};

use crate::instructions::PortalError;

pub type Bytes32 = [u8; 32];

pub fn intent_hash(route_chain: Bytes32, route_hash: Bytes32, reward: &Reward) -> Bytes32 {
    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];

    hasher.update(&route_chain);
    hasher.update(&route_hash);
    hasher.update(&reward.hash());

    hasher.finalize(&mut hash);

    hash
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Intent {
    pub route_chain: Bytes32,
    pub route: Route,
    pub reward: Reward,
}

impl Intent {
    pub fn validate(&self, clock: Clock) -> Result<()> {
        self.route.validate()?;
        self.reward.validate(clock)?;

        Ok(())
    }
}

fn validate_token_amounts(tokens: &[TokenAmount]) -> Result<()> {
    require!(
        tokens
            .iter()
            .map(|token_amount| token_amount.token)
            .collect::<HashSet<_>>()
            .len()
            == tokens.len(),
        PortalError::DuplicateTokens
    );

    tokens.iter().try_for_each(TokenAmount::validate)?;

    Ok(())
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Route {
    pub salt: Bytes32,
    pub route_chain_portal: Bytes32,
    pub tokens: Vec<TokenAmount>,
    pub calls: Vec<Call>,
}

impl Route {
    fn validate(&self) -> Result<()> {
        validate_token_amounts(&self.tokens)?;
        require!(!self.calls.is_empty(), PortalError::EmptyCalls);

        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Reward {
    pub deadline: i64,
    pub creator: Pubkey,
    pub prover: Bytes32,
    pub native_amount: u64,
    pub tokens: Vec<TokenAmount>,
}

impl Reward {
    fn validate(&self, clock: Clock) -> Result<()> {
        require!(
            self.deadline > clock.unix_timestamp,
            PortalError::InvalidIntentDeadline
        );
        validate_token_amounts(&self.tokens)?;

        Ok(())
    }

    fn hash(&self) -> Bytes32 {
        let encoded = self.try_to_vec().expect("Failed to serialize Reward");
        let mut hasher = Keccak::v256();
        let mut hash = [0u8; 32];

        hasher.update(&encoded);
        hasher.finalize(&mut hash);

        hash
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct TokenAmount {
    pub token: Bytes32,
    pub amount: u64,
}

impl TokenAmount {
    fn validate(&self) -> Result<()> {
        require!(self.amount > 0, PortalError::InvalidTokenAmount);

        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Call {
    pub target: Bytes32,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_hash_deterministic() {
        let route_chain = [5u8; 32];
        let route_hash = [6u8; 32];
        let reward = Reward {
            deadline: 1500000,
            creator: Pubkey::default(),
            prover: [7u8; 32],
            native_amount: 250,
            tokens: vec![
                TokenAmount {
                    token: [40u8; 32],
                    amount: 1000,
                },
                TokenAmount {
                    token: [50u8; 32],
                    amount: 2000,
                },
            ],
        };

        let hash_1 = intent_hash(route_chain, route_hash, &reward);
        let hash_2 = intent_hash(route_chain, route_hash, &reward);

        assert_eq!(hash_1, hash_2);
        goldie::assert_json!(hash_1);
    }

    #[test]
    fn intent_validate_success() {
        let clock = Clock {
            unix_timestamp: 1000000,
            slot: 100,
            epoch: 10,
            leader_schedule_epoch: 10,
            epoch_start_timestamp: 900000,
        };

        let intent = Intent {
            route_chain: [1u8; 32],
            route: Route {
                salt: [2u8; 32],
                route_chain_portal: [3u8; 32],
                tokens: vec![TokenAmount {
                    token: [4u8; 32],
                    amount: 100,
                }],
                calls: vec![Call {
                    target: [5u8; 32],
                    data: vec![1, 2, 3],
                }],
            },
            reward: Reward {
                deadline: 2000000,
                creator: Pubkey::new_unique(),
                prover: [6u8; 32],
                native_amount: 50,
                tokens: vec![TokenAmount {
                    token: [7u8; 32],
                    amount: 200,
                }],
            },
        };

        assert!(intent.validate(clock).is_ok());
    }

    #[test]
    fn intent_validate_expired_deadline() {
        let clock = Clock {
            unix_timestamp: 2000000,
            slot: 200,
            epoch: 20,
            leader_schedule_epoch: 20,
            epoch_start_timestamp: 1900000,
        };

        let intent = Intent {
            route_chain: [1u8; 32],
            route: Route {
                salt: [2u8; 32],
                route_chain_portal: [3u8; 32],
                tokens: vec![TokenAmount {
                    token: [4u8; 32],
                    amount: 100,
                }],
                calls: vec![Call {
                    target: [5u8; 32],
                    data: vec![1, 2, 3],
                }],
            },
            reward: Reward {
                deadline: 1500000,
                creator: Pubkey::new_unique(),
                prover: [6u8; 32],
                native_amount: 50,
                tokens: vec![],
            },
        };

        assert!(intent
            .validate(clock)
            .is_err_and(|err| err.to_string().contains("InvalidIntentDeadline")));
    }

    #[test]
    fn intent_validate_empty_calls() {
        let clock = Clock {
            unix_timestamp: 1000000,
            slot: 100,
            epoch: 10,
            leader_schedule_epoch: 10,
            epoch_start_timestamp: 900000,
        };

        let intent = Intent {
            route_chain: [1u8; 32],
            route: Route {
                salt: [2u8; 32],
                route_chain_portal: [3u8; 32],
                tokens: vec![TokenAmount {
                    token: [4u8; 32],
                    amount: 100,
                }],
                calls: vec![],
            },
            reward: Reward {
                deadline: 2000000,
                creator: Pubkey::new_unique(),
                prover: [6u8; 32],
                native_amount: 50,
                tokens: vec![],
            },
        };

        assert!(intent
            .validate(clock)
            .is_err_and(|err| err.to_string().contains("EmptyCalls")));
    }

    #[test]
    fn intent_validate_duplicate_route_tokens() {
        let clock = Clock {
            unix_timestamp: 1000000,
            slot: 100,
            epoch: 10,
            leader_schedule_epoch: 10,
            epoch_start_timestamp: 900000,
        };

        let duplicate_token = [4u8; 32];
        let intent = Intent {
            route_chain: [1u8; 32],
            route: Route {
                salt: [2u8; 32],
                route_chain_portal: [3u8; 32],
                tokens: vec![
                    TokenAmount {
                        token: duplicate_token,
                        amount: 100,
                    },
                    TokenAmount {
                        token: duplicate_token,
                        amount: 200,
                    },
                ],
                calls: vec![Call {
                    target: [5u8; 32],
                    data: vec![1, 2, 3],
                }],
            },
            reward: Reward {
                deadline: 2000000,
                creator: Pubkey::new_unique(),
                prover: [6u8; 32],
                native_amount: 50,
                tokens: vec![],
            },
        };

        assert!(intent
            .validate(clock)
            .is_err_and(|err| err.to_string().contains("DuplicateTokens")));
    }

    #[test]
    fn intent_validate_duplicate_reward_tokens() {
        let clock = Clock {
            unix_timestamp: 1000000,
            slot: 100,
            epoch: 10,
            leader_schedule_epoch: 10,
            epoch_start_timestamp: 900000,
        };

        let duplicate_token = [7u8; 32];
        let intent = Intent {
            route_chain: [1u8; 32],
            route: Route {
                salt: [2u8; 32],
                route_chain_portal: [3u8; 32],
                tokens: vec![TokenAmount {
                    token: [4u8; 32],
                    amount: 100,
                }],
                calls: vec![Call {
                    target: [5u8; 32],
                    data: vec![1, 2, 3],
                }],
            },
            reward: Reward {
                deadline: 2000000,
                creator: Pubkey::new_unique(),
                prover: [6u8; 32],
                native_amount: 50,
                tokens: vec![
                    TokenAmount {
                        token: duplicate_token,
                        amount: 100,
                    },
                    TokenAmount {
                        token: duplicate_token,
                        amount: 200,
                    },
                ],
            },
        };

        assert!(intent
            .validate(clock)
            .is_err_and(|err| err.to_string().contains("DuplicateTokens")));
    }

    #[test]
    fn intent_validate_zero_token_amount() {
        let clock = Clock {
            unix_timestamp: 1000000,
            slot: 100,
            epoch: 10,
            leader_schedule_epoch: 10,
            epoch_start_timestamp: 900000,
        };

        let intent = Intent {
            route_chain: [1u8; 32],
            route: Route {
                salt: [2u8; 32],
                route_chain_portal: [3u8; 32],
                tokens: vec![TokenAmount {
                    token: [4u8; 32],
                    amount: 0,
                }],
                calls: vec![Call {
                    target: [5u8; 32],
                    data: vec![1, 2, 3],
                }],
            },
            reward: Reward {
                deadline: 2000000,
                creator: Pubkey::new_unique(),
                prover: [6u8; 32],
                native_amount: 50,
                tokens: vec![],
            },
        };

        assert!(intent
            .validate(clock)
            .is_err_and(|err| err.to_string().contains("InvalidTokenAmount")));
    }
}
