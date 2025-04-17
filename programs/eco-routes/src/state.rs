use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq)]
pub enum IntentStatus {
    Init,
    PartiallyFunded,
    Funded,
    Fulfilled,
    Claimed,
    Refunded,
}

const MAX_ROUTED_TOKENS: usize = 10;
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq)]
pub struct RoutedTokenData {
    pub mint: Pubkey,
    pub amount: u64,
    pub deposited: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq)]
pub struct RouteData {
    pub salt: [u8; 32],
    pub source_chain_id: u64,
    pub destination_chain_id: u64,
    pub inbox: [u8; 20],
    pub routed_tokens: [RoutedTokenData; MAX_ROUTED_TOKENS],
    // TODO: calls-related fields
}

pub const MAX_REWARD_TOKENS: usize = 10;
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq)]
pub struct TokenRewardData {
    pub mint: Pubkey,
    pub amount: u64,
    pub deposited: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq)]
pub struct RewardData {
    pub creator: Pubkey,
    pub deadline: i64,
    pub native_amount: u64,
    pub native_deposited: u64,
    pub tokens: [TokenRewardData; MAX_REWARD_TOKENS],
    // TODO: prover fields
}

pub const INTENT_SEED: &[u8] = b"intent";
#[account]
#[derive(InitSpace)]
pub struct Intent {
    pub status: IntentStatus,
    pub route_data: RouteData,
    pub reward_data: RewardData,
    // TODO: metadata fields (timestamps, solver, etc...)
}

impl Intent {
    pub fn pda(salt: [u8; 32]) -> Pubkey {
        Pubkey::find_program_address(&[INTENT_SEED, salt.as_ref()], &crate::ID).0
    }
}
