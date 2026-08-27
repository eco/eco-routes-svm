use anchor_lang::prelude::*;

mod chain;

pub use chain::*;

pub fn now() -> Result<u64> {
    Ok(Clock::get()?
        .unix_timestamp
        .try_into()
        .expect("timestamp must fit in u64"))
}

/// Errors emitted by the intent-chainer program.
#[error_code]
pub enum ChainerError {
    /// `segments.len()` is not `slots.len() + 1`.
    SegmentCountMismatch,
    /// `slots.len()` exceeds [`crate::types::MAX_SLOTS`].
    TooManySlots,
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
    /// `amount_in * scale` overflows even after reducing the fraction.
    ScaleOverflow,
    /// A slot width is zero or above 32.
    InvalidSlotWidth,
    /// The destination amount does not fit the slot, e.g. a Solana u64 slot and an
    /// 18-decimal amount.
    AmountExceedsSlotWidth,
    /// Intent2's reward deadline is in the past or inside
    /// [`crate::types::MIN_DEADLINE_BUFFER`].
    DeadlineTooSoon,
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
    /// The `withdrawn_marker` address is not
    /// `portal::state::WithdrawnMarker::pda(intent_hash)`.
    InvalidWithdrawnMarker,
    /// Intent2's reward has already been withdrawn. Pushing more into a settled
    /// vault would be unrecoverable by the claimant.
    IntentAlreadySettled,
    /// The vault holds less than was pushed, e.g. a token-2022 transfer fee.
    PushShortfall,
    /// `portal_program` is not `portal::ID`.
    InvalidPortalProgram,
}
