use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

/// A follow-on intent was resolved from a measured balance and funded.
///
/// The route bytes are deliberately **not** carried here. They are recoverable
/// off-chain without them: the whole `Order` is in this instruction's own data,
/// and the splice is deterministic given `amount_out`, so any indexer that reads
/// the transaction can reconstruct the exact route. Re-emitting it would double
/// the log cost of the `publish` path for no information, and the runtime's
/// per-transaction log budget is the binding constraint on route length
/// (see [`crate::types::MAX_ROUTE_LEN`]). `route_hash` is included so a
/// reconstruction can be checked rather than trusted.
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
    /// `ceil(amount_in * scale / WAD)`, written into every route slot.
    pub amount_out: u128,
    /// Intent2's destination chain id.
    pub destination: u64,
    /// `keccak(route)`, so a reconstructed route can be verified.
    pub route_hash: Bytes32,
    /// Whether this call also emitted portal's canonical `IntentPublished`.
    pub published: bool,
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
