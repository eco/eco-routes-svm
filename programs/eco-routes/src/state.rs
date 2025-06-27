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
    require!(!tokens.is_empty(), EcoRoutesError::EmptyTokens);
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
    require!(!calls.is_empty(), EcoRoutesError::EmptyCalls);

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
    ) -> Result<Self> {
        require!(
            deadline > Clock::get()?.unix_timestamp,
            EcoRoutesError::InvalidDeadline
        );
        validate_token_amounts(&tokens, MAX_REWARD_TOKENS)?;

        Ok(Self {
            tokens,
            native_amount,
            deadline,
            creator,
            prover: crate::ID.to_bytes(),
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

    pub fn spendable_lamports(&self, account_info: &AccountInfo) -> Result<u64> {
        let rent_exemption = Rent::get()?.minimum_balance(8 + Intent::INIT_SPACE);

        Ok(account_info.lamports() - rent_exemption)
    }

    pub fn is_expired(&self) -> Result<bool> {
        Ok(self.reward.deadline < Clock::get()?.unix_timestamp)
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

    pub fn fund_token(&mut self) -> Result<()> {
        let reward_token_count = self.reward.tokens.len() as u8;

        match self.status {
            IntentStatus::Funding(native_funded, funded_token_count)
                if funded_token_count + 1 == reward_token_count && native_funded =>
            {
                self.status = IntentStatus::Funded;
                Ok(())
            }
            IntentStatus::Funding(native_funded, funded_token_count) => {
                self.status = IntentStatus::Funding(native_funded, funded_token_count + 1);
                Ok(())
            }
            _ => Err(EcoRoutesError::NotInFundingPhase.into()),
        }
    }

    pub fn refund_native(&mut self) -> Result<()> {
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

    pub fn refund_token(&mut self) -> Result<()> {
        match self.status {
            IntentStatus::Funding(native_funded, funded_token_count) if funded_token_count > 0 => {
                self.status = IntentStatus::Funding(native_funded, funded_token_count - 1);
                Ok(())
            }
            IntentStatus::Funded => {
                self.status = IntentStatus::Funding(true, self.reward.tokens.len() as u8 - 1);
                Ok(())
            }
            _ => Err(EcoRoutesError::NotInRefundingPhase.into()),
        }
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
            _ => Err(EcoRoutesError::NotFulfilled.into()),
        }
    }

    pub fn claim_token(&mut self) -> Result<()> {
        match self.status {
            IntentStatus::Claimed(native_claimed, claimed_token_count)
                if claimed_token_count + 1 <= self.reward.tokens.len() as u8 =>
            {
                self.status = IntentStatus::Claimed(native_claimed, claimed_token_count + 1);
                Ok(())
            }
            IntentStatus::Fulfilled => {
                self.status = IntentStatus::Claimed(false, 1);
                Ok(())
            }
            _ => Err(EcoRoutesError::NotFulfilled.into()),
        }
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
