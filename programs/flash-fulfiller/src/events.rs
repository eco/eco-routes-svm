use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

/// Emitted when `flash_fulfill` succeeds.
#[event]
pub struct FlashFulfilled {
    /// Hash of the fulfilled intent.
    pub intent_hash: Bytes32,
    /// Recipient of leftover tokens and native SOL.
    pub claimant: Pubkey,
    /// Native SOL swept to the claimant (`reward.native_amount - route.native_amount`).
    pub native_fee: u64,
}
