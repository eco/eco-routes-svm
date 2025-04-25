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
    pub mint: Pubkey,
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
            if !hashset.insert(token.mint) {
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
    pub source_domain_id: u32,
    pub destination_domain_id: u32,
    pub inbox: [u8; 32],
    pub prover: Pubkey,
    pub calls_root: [u8; 32],
    pub route_root: [u8; 32],
    #[max_len(MAX_ROUTE_TOKENS)]
    pub tokens: Vec<TokenAmount>,
    pub tokens_funded: u8,
    #[max_len(MAX_CALLS)]
    pub calls: Vec<Call>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug, InitSpace)]
pub struct Reward {
    #[max_len(MAX_REWARD_TOKENS)]
    pub tokens: Vec<TokenAmount>,
    pub tokens_funded: u8,
    pub native_reward: u64,
    pub native_funded: u64,
}

#[account]
#[derive(PartialEq, Eq, Debug, InitSpace)]
pub struct Intent {
    pub salt: [u8; 32],
    pub intent_hash: [u8; 32],
    pub status: IntentStatus,
    pub creator: Pubkey,
    pub prover: Pubkey,
    pub deadline: i64,
    pub route: Route,
    pub reward: Reward,
    pub solver: Pubkey,
    pub bump: u8,
}

impl Intent {
    pub fn pda(intent_hash: [u8; 32]) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"intent", intent_hash.as_ref()], &crate::ID)
    }

    pub fn is_funded(&self) -> bool {
        self.route.tokens_funded == self.route.tokens.len() as u8
            && self.reward.tokens_funded == self.reward.tokens.len() as u8
            && self.reward.native_funded == self.reward.native_reward
    }

    pub fn is_expired(&self) -> Result<bool> {
        let clock = Clock::get()?;
        Ok(self.deadline < clock.unix_timestamp)
    }

    pub fn is_empty(&self) -> bool {
        self.status == IntentStatus::Funded
            && self.route.tokens_funded == 0
            && self.reward.tokens_funded == 0
            && self.reward.native_funded == 0
    }
}

// assert_eq!(Intent::INIT_SPACE, 1452);

#[account]
#[derive(PartialEq, Eq, Debug, InitSpace)]
pub struct IntentMarker {
    pub bump: u8,
    pub source_domain_id: u32,
    pub intent_hash: [u8; 32],
    pub calls_root: [u8; 32],
    pub route_root: [u8; 32],
    pub deadline: i64,
    pub fulfilled: bool,
}

impl IntentMarker {
    pub fn pda(intent_hash: [u8; 32]) -> Pubkey {
        Pubkey::find_program_address(&[b"intent_marker", intent_hash.as_ref()], &crate::ID).0
    }
}

// assert_eq!(IntentMarker::INIT_SPACE, 110);

pub const MAX_SENDERS_PER_DOMAIN: usize = 128;

#[account]
#[derive(PartialEq, Eq, Debug, InitSpace)]
pub struct DomainRegistry {
    pub origin_domain_id: u32,
    #[max_len(MAX_SENDERS_PER_DOMAIN)]
    pub trusted_senders: Vec<[u8; 32]>,
    pub bump: u8,
}

impl DomainRegistry {
    pub fn pda(origin_domain_id: u32) -> Pubkey {
        Pubkey::find_program_address(
            &[b"domain_registry", &origin_domain_id.to_le_bytes()],
            &crate::ID,
        )
        .0
    }

    pub fn validate(&self) -> Result<()> {
        if self.trusted_senders.len() > MAX_SENDERS_PER_DOMAIN {
            return Err(EcoRoutesError::TooManySenders.into());
        }
        Ok(())
    }

    pub fn is_sender_trusted(&self, origin_domain_id: u32, sender: &[u8; 32]) -> bool {
        if origin_domain_id == self.origin_domain_id {
            return true;
        }
        self.trusted_senders.contains(sender)
    }
}

// assert_eq!(DomainRegistry::INIT_SPACE, 4105);
