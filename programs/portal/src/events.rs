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
}

/// Emitted when a marker is shrunk to its tombstone. `lamports` is what was
/// returned to `payer`; the tombstone keeps `claimant`, recorded here too so
/// the event stream is a complete audit record.
#[event]
#[derive(new)]
pub struct FulfillMarkerClosed {
    intent_hash: Bytes32,
    payer: Pubkey,
    claimant: Bytes32,
    lamports: u64,
}

#[event]
#[derive(new)]
pub struct IntentCancelled {
    intent_hash: Bytes32,
}
