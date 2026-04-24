use anchor_lang::prelude::*;

mod append_flash_fulfill_route_chunk;
mod cancel_flash_fulfill_intent;
mod close_abandoned_flash_fulfill_intent;
mod flash_fulfill;
mod init_flash_fulfill_intent;
mod set_flash_fulfill_intent;

pub use append_flash_fulfill_route_chunk::*;
pub use cancel_flash_fulfill_intent::*;
pub use close_abandoned_flash_fulfill_intent::*;
pub use flash_fulfill::*;
pub use init_flash_fulfill_intent::*;
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
    /// Supplied `intent_hash` does not match `keccak(CHAIN_ID, route_hash, reward.hash())` for the provided preimage.
    InvalidIntentHash,
    /// `route_total_size` is zero or exceeds `MAX_ROUTE_INIT_SPACE`.
    InvalidRouteTotalSize,
    /// Appended `offset` does not match `route_bytes_written`; only strict append is allowed.
    InvalidAppendOffset,
    /// `offset + chunk.len()` would exceed `route_total_size`.
    AppendOverflow,
    /// Buffer is already finalized and cannot be appended to or cancelled.
    BufferAlreadyFinalized,
    /// Buffer is not yet finalized and cannot be consumed.
    BufferNotFinalized,
    /// Keccak256 of the buffered bytes does not match the committed `route_hash`.
    RouteHashMismatch,
    /// The buffered bytes do not Borsh-deserialize as a `Route`.
    RouteDecodeFailed,
    /// Buffer has not exceeded its abandonment TTL yet.
    NotAbandonedYet,
    /// A route call's `data` field is not a well-formed Borsh-encoded
    /// `CalldataWithAccounts`: too short, or declares a `Calldata` length that
    /// exceeds the buffer. Returned by the zero-copy strip in `flash_fulfill`.
    InvalidCallData,
}
