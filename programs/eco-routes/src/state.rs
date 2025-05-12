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
    Funding(bool, u8),
    Funded,
    Fulfilled,
    Claimed(bool, u8),
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

    pub solver: Option<[u8; 32]>,

    pub bump: u8,
}

impl Intent {
    pub fn pda(intent_hash: [u8; 32]) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"intent", intent_hash.as_ref()], &crate::ID)
    }

    pub fn rent_exempt_lamports(&self, account_info: &AccountInfo) -> Result<u64> {
        let rent = Rent::get()?;
        let rent_exemption = rent.minimum_balance(8 + Intent::INIT_SPACE);
        Ok(account_info.lamports() - rent_exemption)
    }

    pub fn is_expired(&self) -> Result<bool> {
        let clock = Clock::get()?;
        Ok(self.reward.deadline < clock.unix_timestamp)
    }

    pub fn fund_native(&mut self) -> Result<()> {
        let num_reward_tokens = self.reward.tokens.len() as u8;
        match self.status {
            IntentStatus::Funding(_, funded_token_count) => {
                if funded_token_count == num_reward_tokens {
                    self.status = IntentStatus::Funded;
                } else {
                    self.status = IntentStatus::Funding(true, funded_token_count);
                }
                Ok(())
            }
            _ => Err(EcoRoutesError::NotInFundingPhase.into()),
        }
    }

    pub fn fund_token(&mut self) -> Result<()> {
        let num_reward_tokens = self.reward.tokens.len() as u8;
        match self.status {
            IntentStatus::Funding(native_funded, funded_token_count) => {
                if funded_token_count + 1 == num_reward_tokens && native_funded {
                    self.status = IntentStatus::Funded;
                } else {
                    self.status = IntentStatus::Funding(native_funded, funded_token_count + 1);
                }
                Ok(())
            }
            _ => Err(EcoRoutesError::NotInFundingPhase.into()),
        }
    }

    pub fn refund_native(&mut self) -> Result<()> {
        let num_reward_tokens = self.reward.tokens.len() as u8;
        match self.status {
            IntentStatus::Funding(_, funded_token_count) => {
                self.status = IntentStatus::Funding(false, funded_token_count);
                Ok(())
            }
            IntentStatus::Funded => {
                self.status = IntentStatus::Funding(false, num_reward_tokens);
                Ok(())
            }
            _ => Err(EcoRoutesError::NotInRefundingPhase.into()),
        }
    }

    pub fn refund_token(&mut self) -> Result<()> {
        let num_reward_tokens = self.reward.tokens.len() as u8;
        match self.status {
            IntentStatus::Funding(native_funded, funded_token_count) => {
                self.status = IntentStatus::Funding(native_funded, funded_token_count - 1);
                Ok(())
            }
            IntentStatus::Funded => {
                self.status = IntentStatus::Funding(true, num_reward_tokens - 1);
                Ok(())
            }
            _ => Err(EcoRoutesError::NotInRefundingPhase.into()),
        }
    }

    pub fn claim_native(&mut self) -> Result<()> {
        match self.status {
            IntentStatus::Claimed(_, claimed_token_count) => {
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
            IntentStatus::Claimed(native_claimed, claimed_token_count) => {
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
