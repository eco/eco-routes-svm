use anchor_lang::prelude::*;

mod flash_fulfill;
mod set_flash_fulfill_intent;

pub use flash_fulfill::*;
pub use set_flash_fulfill_intent::*;

#[error_code]
pub enum FlashFulfillerError {
    InvalidFlashFulfillIntentAccount,
    InvalidClaimant,
    InvalidFlashVault,
    InvalidRemainingAccounts,
    InvalidClaimantToken,
}
