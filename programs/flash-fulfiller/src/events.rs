use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

#[event]
pub struct FlashFulfilled {
    pub intent_hash: Bytes32,
    pub claimant: Pubkey,
    pub native_fee: u64,
}
