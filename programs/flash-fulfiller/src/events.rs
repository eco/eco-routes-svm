use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

/// Emitted when `flash_fulfill` succeeds.
#[event]
pub struct FlashFulfilled {
    /// Hash of the fulfilled intent.
    pub intent_hash: Bytes32,
    /// Recipient of leftover tokens and native SOL.
    pub claimant: Pubkey,
    /// Solver's native SOL profit on this intent: `reward.native_amount -
    /// route.native_amount` (saturating). This is the documented intent
    /// delta, not the actual lamports swept from `flash_vault` to the
    /// claimant — pre-funded lamports on the system-owned PDA are also
    /// swept but are not reflected here.
    pub native_fee: u64,
}
