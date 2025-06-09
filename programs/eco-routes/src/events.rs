use anchor_lang::prelude::*;
use derive_new::new;

use crate::state::{Reward, Route};

#[event]
#[derive(new)]
pub struct IntentCreated {
    intent_hash: [u8; 32],
    route: Route,
    reward: Reward,
}

#[event]
#[derive(new)]
pub struct IntentFundedNative {
    intent_hash: [u8; 32],
}

#[event]
#[derive(new)]
pub struct IntentFundedSpl {
    intent_hash: [u8; 32],
    mint: Pubkey,
}

#[event]
#[derive(new)]
pub struct IntentRefundedNative {
    intent_hash: [u8; 32],
}

#[event]
#[derive(new)]
pub struct IntentRefundedSpl {
    intent_hash: [u8; 32],
    mint: Pubkey,
}

#[event]
#[derive(new)]
pub struct IntentClaimedNative {
    intent_hash: [u8; 32],
}

#[event]
#[derive(new)]
pub struct IntentClaimedSpl {
    intent_hash: [u8; 32],
    mint: Pubkey,
}

#[event]
#[derive(new)]
pub struct IntentClosed {
    intent_hash: [u8; 32],
}

#[event]
#[derive(new)]
pub struct IntentFulfilled {
    intent_hash: [u8; 32],
    source_domain_id: u32,
    prover: [u8; 32],
    solver: [u8; 32],
}
