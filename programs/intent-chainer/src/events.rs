use anchor_lang::prelude::*;
use derive_new::new;
use eco_svm_std::Bytes32;

/// A follow-on intent was resolved from a measured balance and funded.
///
/// Route bytes are reconstructed from the complete Order and BOTH initial amounts.
/// Nested recipients are deterministic data, not local transfer accounts. The
/// route hash checks reconstruction; Portal's optional publication carries the
/// full route. The Order preimage is durably announced through a self-CPI event.
#[event]
pub struct IntentChained {
    /// Intent2's hash, computed from the spliced route and the resolved reward.
    pub intent_hash: Bytes32,
    /// Commitment that seeded the escrow authority; identifies the order.
    pub order_commitment: Bytes32,
    /// Intent2's vault, which now holds `amount_in`.
    pub vault: Pubkey,
    /// The measured mint.
    pub base_mint: Pubkey,
    /// The measured amount, escrowed as intent2's reward.
    pub amount_in: u64,
    /// Initial Output context: `ceil(amount_in * scale / WAD)` (not narrowed to u64).
    pub amount_out: u128,
    /// Intent2's destination chain id.
    pub destination: u64,
    /// `keccak(route)`, so a reconstructed route can be verified.
    pub route_hash: Bytes32,
    /// Whether this call also emitted portal's canonical `IntentPublished`.
    pub published: bool,
}

/// The complete order preimage, recorded as an Anchor self-CPI event.
///
/// Carries the whole order, which is the point: the escrow authority is
/// `keccak(borsh(order))`, so without the preimage a funded escrow has no
/// derivation path and no sweep. See [`crate::instructions::announce_order`].
#[event]
#[derive(new)]
pub struct OrderAnnounced {
    /// `keccak(borsh(order))` — the escrow authority's seed.
    pub order_commitment: Bytes32,
    /// The escrow authority the order derives.
    pub escrow_authority: Pubkey,
    /// The complete nested preimage, including every remote configuration.
    pub order: crate::types::Order,
}

impl IntentChained {
    /// Hand-written rather than `derive_new`: nine fields trips
    /// `clippy::too_many_arguments`, and the lint is worth silencing explicitly
    /// here rather than losing a field an indexer needs. Every one earns its
    /// place — `order_commitment` correlates back to intent1's escrow address,
    /// `route_hash` lets a reconstruction be verified, and `published` tells a
    /// consumer whether to expect portal's `IntentPublished` too.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        intent_hash: Bytes32,
        order_commitment: Bytes32,
        vault: Pubkey,
        base_mint: Pubkey,
        amount_in: u64,
        amount_out: u128,
        destination: u64,
        route_hash: Bytes32,
        published: bool,
    ) -> Self {
        Self {
            intent_hash,
            order_commitment,
            vault,
            base_mint,
            amount_in,
            amount_out,
            destination,
            route_hash,
            published,
        }
    }
}
