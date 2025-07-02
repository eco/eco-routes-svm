use std::collections::HashSet;

use anchor_lang::prelude::*;

use crate::{encoding, error::EcoRoutesError};

pub const MAX_ROUTE_TOKENS: usize = 3;
pub const MAX_REWARD_TOKENS: usize = 3;
pub const MAX_CALLS: usize = 3;
pub const MAX_CALLDATA_SIZE: usize = 256;
pub const ECO_ROUTES_AUTHORITY: &str = "aEGzbWJhZ7RX8uCmeG4jVfskQe6eoP7zcdoHmY2PWys";

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace)]
#[repr(u8)]
pub enum IntentStatus {
    Funding(bool, u8),
    Funded,
    Fulfilled,
    Claimed(bool, u8),
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace, Default)]
pub struct TokenAmount {
    pub token: [u8; 32],
    pub amount: u64,
}

impl TokenAmount {
    fn validate(&self) -> Result<()> {
        require!(self.amount > 0, EcoRoutesError::ZeroTokenAmount);

        Ok(())
    }
}

fn validate_token_amounts(tokens: &[TokenAmount], max_len: usize) -> Result<()> {
    require!(tokens.len() <= max_len, EcoRoutesError::TooManyTokens);
    require!(
        tokens
            .iter()
            .map(|token_amount| token_amount.token)
            .collect::<HashSet<_>>()
            .len()
            == tokens.len(),
        EcoRoutesError::DuplicateTokens
    );

    tokens.iter().try_for_each(TokenAmount::validate)?;

    Ok(())
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace, Default)]
pub struct Call {
    pub destination: [u8; 32],
    #[max_len(MAX_CALLDATA_SIZE)]
    pub calldata: Vec<u8>,
}

impl Call {
    fn validate(&self) -> Result<()> {
        if self.calldata.len() > MAX_CALLDATA_SIZE {
            return Err(EcoRoutesError::CallDataTooLarge.into());
        }

        Ok(())
    }
}

fn validate_calls(calls: &[Call], max_len: usize) -> Result<()> {
    require!(calls.len() <= max_len, EcoRoutesError::TooManyCalls);

    calls.iter().try_for_each(Call::validate)?;

    Ok(())
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace, Default)]
pub struct Route {
    pub salt: [u8; 32],
    pub source_domain_id: u32,
    pub destination_domain_id: u32,
    pub inbox: [u8; 32],
    #[max_len(MAX_ROUTE_TOKENS)]
    pub tokens: Vec<TokenAmount>,
    #[max_len(MAX_CALLS)]
    pub calls: Vec<Call>,
}

impl Route {
    pub fn new(
        salt: [u8; 32],
        destination_domain_id: u32,
        inbox: [u8; 32],
        tokens: Vec<TokenAmount>,
        calls: Vec<Call>,
    ) -> Result<Self> {
        validate_token_amounts(&tokens, MAX_ROUTE_TOKENS)?;
        validate_calls(&calls, MAX_CALLS)?;

        Ok(Self {
            salt,
            source_domain_id: crate::hyperlane::DOMAIN_ID,
            destination_domain_id,
            tokens,
            calls,
            inbox,
        })
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace, Default)]
pub struct Reward {
    pub creator: Pubkey,
    #[max_len(MAX_REWARD_TOKENS)]
    pub tokens: Vec<TokenAmount>,
    pub prover: [u8; 32],
    pub native_amount: u64,
    pub deadline: i64,
}

impl Reward {
    pub fn new(
        tokens: Vec<TokenAmount>,
        creator: Pubkey,
        native_amount: u64,
        deadline: i64,
        clock: Clock,
    ) -> Result<Self> {
        require!(
            deadline > clock.unix_timestamp,
            EcoRoutesError::InvalidDeadline
        );
        validate_token_amounts(&tokens, MAX_REWARD_TOKENS)?;

        Ok(Self {
            creator,
            tokens,
            prover: crate::ID.to_bytes(),
            native_amount,
            deadline,
        })
    }
}

#[account]
#[derive(PartialEq, Eq, Debug, InitSpace)]
pub struct Intent {
    pub intent_hash: [u8; 32],
    pub status: IntentStatus,
    pub route: Route,
    pub reward: Reward,
    pub solver: Option<[u8; 32]>,
    pub bump: u8,
}

impl Intent {
    pub fn new(intent_hash: [u8; 32], route: Route, reward: Reward, bump: u8) -> Result<Self> {
        require!(
            intent_hash == encoding::get_intent_hash(&route, &reward),
            EcoRoutesError::InvalidIntentHash
        );

        Ok(Self {
            intent_hash,
            status: IntentStatus::Funding(false, 0),
            route,
            reward,
            solver: None,
            bump,
        })
    }

    pub fn pda(intent_hash: [u8; 32]) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"intent", intent_hash.as_ref()], &crate::ID)
    }

    pub fn spendable_lamports(rent: Rent, account_info: &AccountInfo) -> u64 {
        account_info
            .lamports()
            .checked_sub(rent.minimum_balance(8 + Intent::INIT_SPACE))
            .unwrap_or_default()
    }

    pub fn is_expired(&self, clock: Clock) -> bool {
        self.reward.deadline <= clock.unix_timestamp
    }

    fn token(&self, token: &[u8; 32]) -> Option<&TokenAmount> {
        self.reward.tokens.iter().find(|t| t.token == *token)
    }

    pub fn fund_native(&mut self) -> Result<()> {
        let reward_token_count = self.reward.tokens.len() as u8;

        match self.status {
            IntentStatus::Funding(false, funded_token_count)
                if funded_token_count == reward_token_count =>
            {
                self.status = IntentStatus::Funded;
                Ok(())
            }
            IntentStatus::Funding(false, funded_token_count) => {
                self.status = IntentStatus::Funding(true, funded_token_count);
                Ok(())
            }
            _ => Err(EcoRoutesError::NotInFundingPhase.into()),
        }
    }

    pub fn fund_token(&mut self, token: &[u8; 32]) -> Result<&TokenAmount> {
        let reward_token_count = self.reward.tokens.len() as u8;

        match self.status {
            IntentStatus::Funding(native_funded, funded_token_count)
                if funded_token_count + 1 == reward_token_count && native_funded =>
            {
                self.status = IntentStatus::Funded;
            }
            IntentStatus::Funding(native_funded, funded_token_count) => {
                self.status = IntentStatus::Funding(native_funded, funded_token_count + 1);
            }
            _ => return Err(EcoRoutesError::NotInFundingPhase.into()),
        }

        Ok(self.token(token).ok_or(EcoRoutesError::InvalidToken)?)
    }

    pub fn refund_native(&mut self, clock: Clock) -> Result<()> {
        require!(self.is_expired(clock), EcoRoutesError::IntentNotExpired);

        match self.status {
            IntentStatus::Funding(true, funded_token_count) => {
                self.status = IntentStatus::Funding(false, funded_token_count);
                Ok(())
            }
            IntentStatus::Funded => {
                self.status = IntentStatus::Funding(false, self.reward.tokens.len() as u8);
                Ok(())
            }
            _ => Err(EcoRoutesError::NotInRefundingPhase.into()),
        }
    }

    pub fn refund_token(&mut self, token: &[u8; 32], clock: Clock) -> Result<&TokenAmount> {
        require!(self.is_expired(clock), EcoRoutesError::IntentNotExpired);

        match self.status {
            IntentStatus::Funding(native_funded, funded_token_count) if funded_token_count > 0 => {
                self.status = IntentStatus::Funding(native_funded, funded_token_count - 1);
            }
            IntentStatus::Funded => {
                self.status = IntentStatus::Funding(true, self.reward.tokens.len() as u8 - 1);
            }
            _ => return Err(EcoRoutesError::NotInRefundingPhase.into()),
        }

        Ok(self.token(token).ok_or(EcoRoutesError::InvalidToken)?)
    }

    pub fn claim_native(&mut self) -> Result<()> {
        match self.status {
            IntentStatus::Claimed(false, claimed_token_count) => {
                self.status = IntentStatus::Claimed(true, claimed_token_count);
                Ok(())
            }
            IntentStatus::Fulfilled => {
                self.status = IntentStatus::Claimed(true, 0);
                Ok(())
            }
            _ => Err(EcoRoutesError::NotClaimable.into()),
        }
    }

    pub fn claim_token(&mut self, token: &[u8; 32]) -> Result<&TokenAmount> {
        match self.status {
            IntentStatus::Claimed(native_claimed, claimed_token_count)
                if claimed_token_count < self.reward.tokens.len() as u8 =>
            {
                self.status = IntentStatus::Claimed(native_claimed, claimed_token_count + 1);
            }
            IntentStatus::Fulfilled => {
                self.status = IntentStatus::Claimed(false, 1);
            }
            _ => return Err(EcoRoutesError::NotClaimable.into()),
        }

        Ok(self.token(token).ok_or(EcoRoutesError::InvalidToken)?)
    }

    pub fn fulfill(&mut self, solver: [u8; 32]) -> Result<()> {
        match self.status {
            IntentStatus::Fulfilled | IntentStatus::Claimed(_, _) => {
                Err(EcoRoutesError::AlreadyFulfilled.into())
            }
            _ => {
                self.status = IntentStatus::Fulfilled;
                self.solver = Some(solver);

                Ok(())
            }
        }
    }
}

#[account]
#[derive(PartialEq, Eq, Debug, InitSpace)]
pub struct IntentFulfillmentMarker {
    pub intent_hash: [u8; 32],
    pub bump: u8,
}

impl IntentFulfillmentMarker {
    pub fn pda(intent_hash: [u8; 32]) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"intent_fulfillment_marker", intent_hash.as_ref()],
            &crate::ID,
        )
    }
}

#[account]
#[derive(PartialEq, Eq, Debug, InitSpace)]
pub struct EcoRoutes {
    pub authority: Pubkey,
    pub prover: [u8; 32],
    pub bump: u8,
}

impl EcoRoutes {
    pub fn new(authority: Pubkey, prover: [u8; 32], bump: u8) -> Self {
        Self {
            authority,
            prover,
            bump,
        }
    }

    pub fn set_authority(&mut self, new_authority: Pubkey) -> Result<()> {
        self.authority = new_authority;

        Ok(())
    }

    pub fn set_authorized_prover(&mut self, new_prover: [u8; 32]) -> Result<()> {
        self.prover = new_prover;

        Ok(())
    }

    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"eco_routes"], &crate::ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_new_should_create_valid_route() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(
            salt,
            destination_domain_id,
            inbox,
            token_amounts.clone(),
            calls.clone(),
        )
        .unwrap();
        assert_eq!(route.salt, salt);
        assert_eq!(route.source_domain_id, crate::hyperlane::DOMAIN_ID);
        assert_eq!(route.destination_domain_id, destination_domain_id);
        assert_eq!(route.inbox, inbox);
        assert_eq!(route.tokens, token_amounts);
        assert_eq!(route.calls, calls);
    }

    #[test]
    fn route_new_should_fail_with_invalid_token_amounts() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![
            TokenAmount {
                token: [2; 32],
                amount: 100,
            },
            TokenAmount {
                token: [5; 32],
                amount: 100,
            },
            TokenAmount {
                token: [6; 32],
                amount: 100,
            },
            TokenAmount {
                token: [7; 32],
                amount: 100,
            },
        ];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        assert_eq!(
            Route::new(
                salt,
                destination_domain_id,
                inbox,
                token_amounts,
                calls.clone(),
            )
            .unwrap_err(),
            EcoRoutesError::TooManyTokens.into()
        );

        let token_amounts = vec![
            TokenAmount {
                token: [2; 32],
                amount: 100,
            },
            TokenAmount {
                token: [2; 32],
                amount: 200,
            },
        ];
        assert_eq!(
            Route::new(
                salt,
                destination_domain_id,
                inbox,
                token_amounts,
                calls.clone(),
            )
            .unwrap_err(),
            EcoRoutesError::DuplicateTokens.into()
        );

        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 0,
        }];
        assert_eq!(
            Route::new(salt, destination_domain_id, inbox, token_amounts, calls,).unwrap_err(),
            EcoRoutesError::ZeroTokenAmount.into()
        );
    }

    #[test]
    fn route_new_should_fail_with_invalid_calls() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];

        let calls = vec![
            Call {
                destination: [3; 32],
                calldata: vec![1, 2, 3],
            },
            Call {
                destination: [4; 32],
                calldata: vec![4, 5, 6],
            },
            Call {
                destination: [5; 32],
                calldata: vec![7, 8, 9],
            },
            Call {
                destination: [6; 32],
                calldata: vec![10, 11, 12],
            },
        ];
        assert_eq!(
            Route::new(
                salt,
                destination_domain_id,
                inbox,
                token_amounts.clone(),
                calls,
            )
            .unwrap_err(),
            EcoRoutesError::TooManyCalls.into()
        );

        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![0; MAX_CALLDATA_SIZE + 1],
        }];
        assert_eq!(
            Route::new(salt, destination_domain_id, inbox, token_amounts, calls,).unwrap_err(),
            EcoRoutesError::CallDataTooLarge.into()
        );
    }

    #[test]
    fn reward_new_should_create_valid_reward() {
        let tokens = vec![TokenAmount {
            token: [1; 32],
            amount: 100,
        }];
        let creator = Pubkey::new_unique();
        let native_amount = 50;
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let deadline = 1609459300;

        let reward = Reward::new(tokens.clone(), creator, native_amount, deadline, clock).unwrap();

        assert_eq!(reward.tokens, tokens);
        assert_eq!(reward.creator, creator);
        assert_eq!(reward.native_amount, native_amount);
        assert_eq!(reward.deadline, deadline);
        assert_eq!(reward.prover, crate::ID.to_bytes());
    }

    #[test]
    fn reward_new_should_fail_with_past_deadline() {
        let tokens = vec![TokenAmount {
            token: [1; 32],
            amount: 100,
        }];
        let creator = Pubkey::new_unique();
        let native_amount = 50;
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let deadline = clock.unix_timestamp - 100;

        assert_eq!(
            Reward::new(tokens, creator, native_amount, deadline, clock).unwrap_err(),
            EcoRoutesError::InvalidDeadline.into()
        );
    }

    #[test]
    fn reward_new_should_fail_with_invalid_tokens() {
        let creator = Pubkey::new_unique();
        let native_amount = 50;
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let deadline = 1609459300;

        let tokens = vec![
            TokenAmount {
                token: [1; 32],
                amount: 100,
            },
            TokenAmount {
                token: [2; 32],
                amount: 200,
            },
            TokenAmount {
                token: [3; 32],
                amount: 300,
            },
            TokenAmount {
                token: [4; 32],
                amount: 400,
            },
        ];
        assert_eq!(
            Reward::new(tokens, creator, native_amount, deadline, clock.clone()).unwrap_err(),
            EcoRoutesError::TooManyTokens.into()
        );

        let tokens = vec![
            TokenAmount {
                token: [1; 32],
                amount: 100,
            },
            TokenAmount {
                token: [1; 32],
                amount: 200,
            },
        ];
        assert_eq!(
            Reward::new(tokens, creator, native_amount, deadline, clock.clone()).unwrap_err(),
            EcoRoutesError::DuplicateTokens.into()
        );

        let tokens = vec![TokenAmount {
            token: [1; 32],
            amount: 0,
        }];
        assert_eq!(
            Reward::new(tokens, creator, native_amount, deadline, clock).unwrap_err(),
            EcoRoutesError::ZeroTokenAmount.into()
        );
    }

    #[test]
    fn intent_new_should_create_valid_intent() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let bump = 123;

        let intent = Intent::new(intent_hash, route.clone(), reward.clone(), bump).unwrap();

        assert_eq!(intent.intent_hash, intent_hash);
        assert_eq!(intent.status, IntentStatus::Funding(false, 0));
        assert_eq!(intent.route, route);
        assert_eq!(intent.reward, reward);
        assert_eq!(intent.solver, None);
        assert_eq!(intent.bump, bump);
    }

    #[test]
    fn intent_new_should_fail_with_invalid_intent_hash() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let wrong_intent_hash = [255; 32];
        let bump = 123;

        assert_eq!(
            Intent::new(wrong_intent_hash, route, reward, bump).unwrap_err(),
            EcoRoutesError::InvalidIntentHash.into()
        );
    }

    #[test]
    fn intent_pda_should_derive_consistent_address() {
        goldie::assert_json!(Intent::pda([1; 32]));
    }

    #[test]
    fn intent_fulfillment_marker_pda_should_derive_consistent_address() {
        goldie::assert_json!(IntentFulfillmentMarker::pda([1; 32]));
    }

    #[test]
    fn spendable_lamports_should_handle_different_balances() {
        let rent = Rent {
            lamports_per_byte_year: 3480,
            exemption_threshold: 2.0,
            burn_percent: 50,
        };
        let rent_exemption = rent.minimum_balance(8 + Intent::INIT_SPACE);
        let key = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let mut data = [];

        let mut lamports = 100_000_000;
        let account = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports,
            &mut data,
            &owner,
            false,
            0,
        );
        assert_eq!(
            Intent::spendable_lamports(rent.clone(), &account),
            lamports - rent_exemption
        );

        let mut lamports = rent_exemption;
        let account = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports,
            &mut data,
            &owner,
            false,
            0,
        );
        assert_eq!(Intent::spendable_lamports(rent.clone(), &account), 0);

        let mut lamports = 1000;
        let account = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports,
            &mut data,
            &owner,
            false,
            0,
        );
        assert_eq!(Intent::spendable_lamports(rent, &account), 0);
    }

    #[test]
    fn is_expired_should_handle_different_deadlines() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let deadline = 1609459300;
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: deadline - 100,
        };
        let reward = Reward::new(reward_tokens, creator, 50, deadline, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: deadline - 50,
        };
        assert!(!intent.is_expired(clock));

        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: deadline + 100,
        };
        assert!(intent.is_expired(clock));

        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: deadline,
        };
        assert!(intent.is_expired(clock));
    }

    #[test]
    fn fund_native_should_transition_funding_states() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![
            TokenAmount {
                token: [5; 32],
                amount: 200,
            },
            TokenAmount {
                token: [6; 32],
                amount: 300,
            },
        ];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        assert_eq!(intent.status, IntentStatus::Funding(false, 0));

        intent.fund_native().unwrap();
        assert_eq!(intent.status, IntentStatus::Funding(true, 0));

        assert_eq!(
            intent.fund_native().unwrap_err(),
            EcoRoutesError::NotInFundingPhase.into()
        );

        intent.status = IntentStatus::Funding(false, 2);
        intent.fund_native().unwrap();
        assert_eq!(intent.status, IntentStatus::Funded);
    }

    #[test]
    fn fund_native_should_fail_when_not_in_funding_phase() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        intent.status = IntentStatus::Funded;
        assert_eq!(
            intent.fund_native().unwrap_err(),
            EcoRoutesError::NotInFundingPhase.into()
        );

        intent.status = IntentStatus::Fulfilled;
        assert_eq!(
            intent.fund_native().unwrap_err(),
            EcoRoutesError::NotInFundingPhase.into()
        );

        intent.status = IntentStatus::Claimed(true, 1);
        assert_eq!(
            intent.fund_native().unwrap_err(),
            EcoRoutesError::NotInFundingPhase.into()
        );
    }

    #[test]
    fn fund_token_should_transition_funding_states() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![
            TokenAmount {
                token: [5; 32],
                amount: 200,
            },
            TokenAmount {
                token: [6; 32],
                amount: 300,
            },
        ];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        assert_eq!(intent.status, IntentStatus::Funding(false, 0));

        let token_amount = intent.fund_token(&[5; 32]).unwrap();
        assert_eq!(token_amount.token, [5; 32]);
        assert_eq!(token_amount.amount, 200);
        assert_eq!(intent.status, IntentStatus::Funding(false, 1));

        intent.status = IntentStatus::Funding(true, 1);
        let token_amount = intent.fund_token(&[6; 32]).unwrap();
        assert_eq!(token_amount.token, [6; 32]);
        assert_eq!(token_amount.amount, 300);
        assert_eq!(intent.status, IntentStatus::Funded);

        intent.status = IntentStatus::Funding(false, 0);
        intent.fund_token(&[5; 32]).unwrap();
        assert_eq!(intent.status, IntentStatus::Funding(false, 1));

        intent.status = IntentStatus::Funding(false, 1);
        intent.fund_token(&[6; 32]).unwrap();
        assert_eq!(intent.status, IntentStatus::Funding(false, 2));
    }

    #[test]
    fn fund_token_should_fail_when_not_in_funding_phase() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        intent.status = IntentStatus::Funded;
        assert_eq!(
            intent.fund_token(&[5; 32]).unwrap_err(),
            EcoRoutesError::NotInFundingPhase.into()
        );

        intent.status = IntentStatus::Fulfilled;
        assert_eq!(
            intent.fund_token(&[5; 32]).unwrap_err(),
            EcoRoutesError::NotInFundingPhase.into()
        );

        intent.status = IntentStatus::Claimed(true, 1);
        assert_eq!(
            intent.fund_token(&[5; 32]).unwrap_err(),
            EcoRoutesError::NotInFundingPhase.into()
        );
    }

    #[test]
    fn fund_token_should_fail_with_invalid_token() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        assert_eq!(
            intent.fund_token(&[99; 32]).unwrap_err(),
            EcoRoutesError::InvalidToken.into()
        );
    }

    #[test]
    fn refund_native_should_transition_refunding_states() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let clock = Clock {
            slot: 200,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459400,
        };

        intent.status = IntentStatus::Funding(true, 0);
        intent.refund_native(clock.clone()).unwrap();
        assert_eq!(intent.status, IntentStatus::Funding(false, 0));

        intent.status = IntentStatus::Funded;
        intent.refund_native(clock).unwrap();
        assert_eq!(intent.status, IntentStatus::Funding(false, 1));
    }

    #[test]
    fn refund_native_should_fail_when_not_expired() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let not_expired_clock = Clock {
            slot: 150,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459250,
        };

        intent.status = IntentStatus::Funding(true, 0);
        assert_eq!(
            intent.refund_native(not_expired_clock).unwrap_err(),
            EcoRoutesError::IntentNotExpired.into()
        );
    }

    #[test]
    fn refund_native_should_fail_when_not_in_refunding_phase() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let clock = Clock {
            slot: 200,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459400,
        };

        intent.status = IntentStatus::Funding(false, 0);
        assert_eq!(
            intent.refund_native(clock.clone()).unwrap_err(),
            EcoRoutesError::NotInRefundingPhase.into()
        );

        intent.status = IntentStatus::Fulfilled;
        assert_eq!(
            intent.refund_native(clock.clone()).unwrap_err(),
            EcoRoutesError::NotInRefundingPhase.into()
        );

        intent.status = IntentStatus::Claimed(true, 1);
        assert_eq!(
            intent.refund_native(clock).unwrap_err(),
            EcoRoutesError::NotInRefundingPhase.into()
        );
    }

    #[test]
    fn refund_token_should_transition_refunding_states() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![
            TokenAmount {
                token: [5; 32],
                amount: 200,
            },
            TokenAmount {
                token: [6; 32],
                amount: 300,
            },
        ];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let clock = Clock {
            slot: 200,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459400,
        };

        intent.status = IntentStatus::Funding(false, 1);
        let token_amount = intent.refund_token(&[5; 32], clock.clone()).unwrap();
        assert_eq!(token_amount.token, [5; 32]);
        assert_eq!(token_amount.amount, 200);
        assert_eq!(intent.status, IntentStatus::Funding(false, 0));

        intent.status = IntentStatus::Funded;
        let token_amount = intent.refund_token(&[6; 32], clock).unwrap();
        assert_eq!(token_amount.token, [6; 32]);
        assert_eq!(token_amount.amount, 300);
        assert_eq!(intent.status, IntentStatus::Funding(true, 1));
    }

    #[test]
    fn refund_token_should_fail_when_not_expired() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let not_expired_clock = Clock {
            slot: 150,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459250,
        };

        intent.status = IntentStatus::Funding(false, 1);
        assert_eq!(
            intent
                .refund_token(&[5; 32], not_expired_clock)
                .unwrap_err(),
            EcoRoutesError::IntentNotExpired.into()
        );
    }

    #[test]
    fn refund_token_should_fail_when_not_in_refunding_phase() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let clock = Clock {
            slot: 200,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459400,
        };

        intent.status = IntentStatus::Funding(false, 0);
        assert_eq!(
            intent.refund_token(&[5; 32], clock.clone()).unwrap_err(),
            EcoRoutesError::NotInRefundingPhase.into()
        );

        intent.status = IntentStatus::Fulfilled;
        assert_eq!(
            intent.refund_token(&[5; 32], clock.clone()).unwrap_err(),
            EcoRoutesError::NotInRefundingPhase.into()
        );

        intent.status = IntentStatus::Claimed(true, 1);
        assert_eq!(
            intent.refund_token(&[5; 32], clock).unwrap_err(),
            EcoRoutesError::NotInRefundingPhase.into()
        );
    }

    #[test]
    fn refund_token_should_fail_with_invalid_token() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let clock = Clock {
            slot: 200,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459400,
        };

        intent.status = IntentStatus::Funding(false, 1);
        assert_eq!(
            intent.refund_token(&[99; 32], clock).unwrap_err(),
            EcoRoutesError::InvalidToken.into()
        );
    }

    #[test]
    fn claim_native_should_transition_claiming_states() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        intent.status = IntentStatus::Claimed(false, 0);
        intent.claim_native().unwrap();
        assert_eq!(intent.status, IntentStatus::Claimed(true, 0));

        intent.status = IntentStatus::Fulfilled;
        intent.claim_native().unwrap();
        assert_eq!(intent.status, IntentStatus::Claimed(true, 0));
    }

    #[test]
    fn claim_native_should_fail_when_not_claimable() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        intent.status = IntentStatus::Funding(false, 0);
        assert_eq!(
            intent.claim_native().unwrap_err(),
            EcoRoutesError::NotClaimable.into()
        );

        intent.status = IntentStatus::Funded;
        assert_eq!(
            intent.claim_native().unwrap_err(),
            EcoRoutesError::NotClaimable.into()
        );

        intent.status = IntentStatus::Claimed(true, 0);
        assert_eq!(
            intent.claim_native().unwrap_err(),
            EcoRoutesError::NotClaimable.into()
        );
    }

    #[test]
    fn claim_token_should_transition_claiming_states() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![
            TokenAmount {
                token: [5; 32],
                amount: 200,
            },
            TokenAmount {
                token: [6; 32],
                amount: 300,
            },
        ];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        intent.status = IntentStatus::Claimed(false, 0);
        let token_amount = intent.claim_token(&[5; 32]).unwrap();
        assert_eq!(token_amount.token, [5; 32]);
        assert_eq!(token_amount.amount, 200);
        assert_eq!(intent.status, IntentStatus::Claimed(false, 1));

        intent.status = IntentStatus::Fulfilled;
        let token_amount = intent.claim_token(&[5; 32]).unwrap();
        assert_eq!(token_amount.token, [5; 32]);
        assert_eq!(token_amount.amount, 200);
        assert_eq!(intent.status, IntentStatus::Claimed(false, 1));
    }

    #[test]
    fn claim_token_should_fail_when_not_claimable() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        intent.status = IntentStatus::Funding(false, 0);
        assert_eq!(
            intent.claim_token(&[5; 32]).unwrap_err(),
            EcoRoutesError::NotClaimable.into()
        );

        intent.status = IntentStatus::Funded;
        assert_eq!(
            intent.claim_token(&[5; 32]).unwrap_err(),
            EcoRoutesError::NotClaimable.into()
        );

        intent.status = IntentStatus::Claimed(false, 1);
        assert_eq!(
            intent.claim_token(&[5; 32]).unwrap_err(),
            EcoRoutesError::NotClaimable.into()
        );
    }

    #[test]
    fn claim_token_should_fail_with_invalid_token() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();

        intent.status = IntentStatus::Fulfilled;
        assert_eq!(
            intent.claim_token(&[99; 32]).unwrap_err(),
            EcoRoutesError::InvalidToken.into()
        );
    }

    #[test]
    fn fulfill_should_transition_to_fulfilled_state() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let solver = [42; 32];

        intent.status = IntentStatus::Funding(false, 0);
        intent.fulfill(solver).unwrap();
        assert_eq!(intent.status, IntentStatus::Fulfilled);
        assert_eq!(intent.solver, Some(solver));

        intent.status = IntentStatus::Funded;
        intent.solver = None;
        intent.fulfill(solver).unwrap();
        assert_eq!(intent.status, IntentStatus::Fulfilled);
        assert_eq!(intent.solver, Some(solver));
    }

    #[test]
    fn fulfill_should_fail_when_already_fulfilled_or_claimed() {
        let salt = [1; 32];
        let destination_domain_id = 1;
        let inbox = [4; 32];
        let token_amounts = vec![TokenAmount {
            token: [2; 32],
            amount: 100,
        }];
        let calls = vec![Call {
            destination: [3; 32],
            calldata: vec![3, 4, 5],
        }];
        let route = Route::new(salt, destination_domain_id, inbox, token_amounts, calls).unwrap();
        let creator = Pubkey::new_unique();
        let reward_tokens = vec![TokenAmount {
            token: [5; 32],
            amount: 200,
        }];
        let clock = Clock {
            slot: 100,
            epoch_start_timestamp: 1000000,
            epoch: 5,
            leader_schedule_epoch: 5,
            unix_timestamp: 1609459200,
        };
        let reward = Reward::new(reward_tokens, creator, 50, 1609459300, clock).unwrap();
        let intent_hash = encoding::get_intent_hash(&route, &reward);
        let mut intent = Intent::new(intent_hash, route, reward, 123).unwrap();
        let solver = [42; 32];

        intent.status = IntentStatus::Fulfilled;
        assert_eq!(
            intent.fulfill(solver).unwrap_err(),
            EcoRoutesError::AlreadyFulfilled.into()
        );

        intent.status = IntentStatus::Claimed(true, 1);
        assert_eq!(
            intent.fulfill(solver).unwrap_err(),
            EcoRoutesError::AlreadyFulfilled.into()
        );
    }
}
