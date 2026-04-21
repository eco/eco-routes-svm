use anchor_lang::prelude::*;

mod flash_fulfill;
mod set_flash_fulfill_intent;

pub use flash_fulfill::*;
pub use set_flash_fulfill_intent::*;

/// Errors emitted by the flash-fulfiller program.
#[error_code]
pub enum FlashFulfillerError {
    /// The `flash_fulfill_intent` account's address does not match the PDA for the supplied intent hash.
    InvalidFlashFulfillIntentAccount,
    /// The claimant pubkey is zero (default) and cannot receive leftover value.
    InvalidClaimant,
    /// The `flash_vault` account's address does not match `flash_vault_pda()`.
    InvalidFlashVault,
    /// Not enough remaining accounts were supplied for the reward/route/claimant transfer triples.
    InvalidRemainingAccounts,
    /// A claimant ATA does not match the canonical ATA for the claimant + mint, or its owner does not match the claimant.
    InvalidClaimantToken,
}
