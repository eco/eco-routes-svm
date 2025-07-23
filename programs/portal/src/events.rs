use anchor_lang::prelude::*;
use derive_new::new;
use eco_svm_std::Bytes32;

use crate::types::Reward;

#[event]
#[derive(new)]
pub struct IntentPublished {
    intent_hash: Bytes32,
    destination: u64,
    route: Vec<u8>,
    reward: Reward,
}

#[event]
#[derive(new)]
pub struct IntentFunded {
    intent_hash: Bytes32,
    funder: Pubkey,
    complete: bool,
}

#[event]
#[derive(new)]
pub struct IntentRefunded {
    intent_hash: Bytes32,
    refundee: Pubkey,
}

#[event]
#[derive(new)]
pub struct IntentWithdrawn {
    intent_hash: Bytes32,
    claimant: Pubkey,
}

#[event]
#[derive(new)]
pub struct IntentFulfilled {
    intent_hash: Bytes32,
    claimant: Bytes32,
}

#[event]
#[derive(new)]
pub struct IntentProven {
    intent_hash: Bytes32,
    claimant: Bytes32,
    source: u64,
}
