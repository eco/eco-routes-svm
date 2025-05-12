use std::collections::HashSet;

use anchor_lang::prelude::*;

use crate::error::EcoRoutesError;

pub const MAX_ROUTE_TOKENS: usize = 3;
pub const MAX_REWARD_TOKENS: usize = 3;
pub const MAX_CALLS: usize = 3;
pub const MAX_CALLDATA_SIZE: usize = 256;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace)]
#[repr(u8)]
pub enum IntentStatus {
    Initialized,
    Funded,
    Dispatched,
    Fulfilled,
    Refunded,
    Claimed,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace)]
pub struct TokenAmount {
    pub token: [u8; 32],
    pub amount: u64,
}

pub trait ValidateTokenList {
    fn validate(&self, max_tokens: usize) -> Result<()>;
}

impl ValidateTokenList for Vec<TokenAmount> {
    fn validate(&self, max_tokens: usize) -> Result<()> {
        if self.len() > max_tokens {
            return Err(EcoRoutesError::TooManyTokens.into());
        }
        let mut hashset = HashSet::new();
        for token in self {
            if !hashset.insert(token.token) {
                return Err(EcoRoutesError::DuplicateTokens.into());
            }
        }
        Ok(())
    }
}
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace)]
pub struct Call {
    pub destination: [u8; 32],
    #[max_len(MAX_CALLDATA_SIZE)]
    pub calldata: Vec<u8>,
}

impl Call {
    pub fn validate(&self) -> Result<()> {
        if self.calldata.len() > MAX_CALLDATA_SIZE {
            return Err(EcoRoutesError::CallDataTooLarge.into());
        }
        Ok(())
    }
}

pub trait ValidateCallList {
    fn validate(&self, max_calls: usize) -> Result<()>;
}

impl ValidateCallList for Vec<Call> {
    fn validate(&self, max_calls: usize) -> Result<()> {
        if self.len() > max_calls {
            return Err(EcoRoutesError::TooManyCalls.into());
        }
        for call in self {
            call.validate()?;
        }
        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace)]
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

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace)]
pub struct Reward {
    pub creator: Pubkey,
    #[max_len(MAX_REWARD_TOKENS)]
    pub tokens: Vec<TokenAmount>,
    pub prover: [u8; 32],
    pub native_amount: u64,
    pub deadline: i64,
}

#[account]
#[derive(PartialEq, Eq, Debug, InitSpace)]
pub struct Intent {
    pub intent_hash: [u8; 32],

    pub status: IntentStatus,

    pub route: Route,
    pub reward: Reward,

    pub tokens_funded: u8,
    pub native_funded: bool,

    pub solver: [u8; 32],

    pub bump: u8,
}

impl Intent {
    pub fn pda(intent_hash: [u8; 32]) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"intent", intent_hash.as_ref()], &crate::ID)
    }

    pub fn is_funded(&self) -> bool {
        self.tokens_funded == self.reward.tokens.len() as u8 && self.native_funded
    }

    pub fn is_expired(&self) -> Result<bool> {
        let clock = Clock::get()?;
        Ok(self.reward.deadline < clock.unix_timestamp)
    }

    pub fn is_empty(&self) -> bool {
        self.tokens_funded == 0 && !self.native_funded
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
