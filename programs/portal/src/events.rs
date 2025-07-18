use anchor_lang::prelude::*;
use derive_new::new;
use eco_svm_std::Bytes32;

use crate::types::{Reward, Route};

#[event]
#[derive(new)]
pub struct IntentPublished {
    intent_hash: Bytes32,
    route: Route,
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
    source: u64,
    destination: u64,
}
