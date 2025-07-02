use anchor_lang::prelude::*;
use derive_new::new;
use eco_svm_std::Bytes32;

#[event]
#[derive(new)]
pub struct IntentFulfilled {
    intent_hash: Bytes32,
    claimant: Bytes32,
}
