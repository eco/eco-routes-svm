use anchor_lang::prelude::*;

mod announce_order;
mod chain;
mod order_buffer;

pub use announce_order::*;
pub use chain::*;
pub use order_buffer::*;

/// Errors emitted by the intent-chainer program.
#[error_code]
pub enum ChainerError {
    /// `segments.len()` is not `items.len() + 1`.
    SegmentCountMismatch,
    /// `items.len()` exceeds [`crate::types::MAX_ITEMS`].
    TooManyItems,
    /// The spliced route exceeds [`crate::types::MAX_ROUTE_LEN`].
    RouteTooLong,
    /// The reward does not carry exactly one leg.
    InvalidRewardLegCount,
    /// The reward leg names a mint this program did not measure.
    RewardTokenMismatch,
    /// The reward leg's `amount` was not authored as zero. Requiring zero is what
    /// makes the commitment preimage canonical.
    RewardAmountMustBeZero,
    /// The reward declares a native amount. Native rewards are out of scope.
    NativeRewardNotSupported,
    /// Nothing was measured — intent1 delivered no `base_mint` to the escrow.
    ZeroAmount,
    /// The measured amount is below the order's floor.
    AmountBelowFloor,
    /// `scale` is zero.
    InvalidScale,
    /// The scaled quotient (including ceil rounding) exceeds u128.
    ScaleOverflow,
    /// An amount width is zero or above 32.
    InvalidAmountWidth,
    /// The destination amount does not fit the item width, e.g. a Solana u64 slot and an
    /// 18-decimal amount.
    AmountExceedsWidth,
    /// The `escrow_authority` address does not match the order's commitment.
    InvalidEscrowAuthority,
    /// The `escrow_ata` is not the escrow authority's derived ATA for `base_mint`.
    InvalidEscrowAta,
    /// The `base_mint` account does not match `order.base_mint`.
    InvalidMint,
    /// The `vault` address is not `portal::state::vault_pda(intent_hash)`.
    InvalidVault,
    /// The `vault_ata` is not the vault's derived ATA for `base_mint`.
    InvalidVaultAta,
    /// Reserved legacy code, no longer emitted. Keep subsequent error codes stable.
    InvalidWithdrawnMarker,
    /// Reserved legacy code, no longer emitted. `chain` does not consult intent2's
    /// `WithdrawnMarker`: a push into a settled vault is refundable by
    /// `reward.creator`, while refusing it would strand the escrow for good. See
    /// [`chain`]. Keep subsequent error codes stable.
    IntentAlreadySettled,
    /// The vault's balance increase differs from the measured transfer amount.
    PushShortfall,
    /// Reserved legacy code, no longer emitted. Keep subsequent error codes stable.
    VaultAlreadyFunded,
    /// `portal_program` does not match the committed `Order.portal`.
    InvalidPortalProgram,
    /// Dependency graph exceeds MAX_VAULTS.
    TooManyVaults,
    /// A self, forward or missing vault reference (also excludes cycles).
    InvalidVaultReference,
    /// Aggregate node route/reward plus root output exceeds the rendering cap.
    RenderedBytesExceeded,
    /// Canonical Borsh preimage exceeds MAX_ORDER_BYTES.
    OrderTooLarge,
    MissingRemotePortal,
    MissingCreate2Prefix,
    MissingImplementation,
    MissingInitCodeHash,
    MissingTokenProgram,
    MissingRemoteMint,
    /// Buffer length, chunk length, or contiguous write offset is invalid.
    InvalidOrderBufferWrite,
    OrderBufferSealed,
    OrderBufferNotSealed,
    OrderBufferIncomplete,
    /// Must decode exactly one canonical bounded Order, without trailing bytes.
    InvalidBufferedOrder,
    OrderCommitmentMismatch,
}
