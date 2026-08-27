use anchor_lang::prelude::*;

use crate::events::OrderAnnounced;
use crate::instructions::ChainerError;
use crate::state::escrow_authority_pda;
use crate::types::{Order, MAX_ROUTE_LEN, MAX_SLOTS};

/// Args for [`announce_order`].
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AnnounceOrderArgs {
    /// The order to put on the record.
    pub order: Order,
}

/// Accounts for [`announce_order`]. None: this only emits.
#[derive(Accounts)]
pub struct AnnounceOrder {}

/// Puts an order's preimage on-chain, so the escrow it derives can always be
/// reached again.
///
/// # Why this exists
///
/// This closes the one place the port is structurally riskier than the EVM
/// contract. There the whole order rides inside `intent1.route.calls[k].data`, so
/// `IntentPublished` records it on-chain forever as a side effect of intent1
/// existing. Here the order travels only in `chain`'s own instruction data — and
/// if `chain` is never called, the order was never on-chain at all.
///
/// That matters because custody is `escrow_authority_pda(keccak(borsh(order)))`.
/// An indexer watching intent1 can see the escrow address, but there is no path
/// from the address back to the order, and no sweep that does not need the order
/// to derive its signer. Losing the preimage strands the balance permanently.
///
/// Calling this in the same transaction that publishes intent1 restores the EVM
/// property: the order is durable public data, recoverable by anyone, and the
/// escrow can always be resolved by re-running `chain`. It is permissionless and
/// takes no accounts, because an announcement grants nothing — `chain` was
/// already permissionless and its outcome is fixed by the order.
///
/// Announcing is not a precondition of `chain`; an SDK that durably keeps its own
/// orders needs neither. It is the on-chain option for those that would rather not
/// carry that liability.
pub fn announce_order(_: Context<AnnounceOrder>, args: AnnounceOrderArgs) -> Result<()> {
    let AnnounceOrderArgs { order } = args;

    // Cheap shape checks only, and before the commitment hash for the same reason
    // `chain` orders them that way: hashing is proportional to the order's size.
    // An announcement is a claim about an escrow, so a malformed order — one no
    // `chain` call could ever consume — should not get a record.
    require!(order.slots.len() <= MAX_SLOTS, ChainerError::TooManySlots);
    require!(
        order.segments.len() == order.slots.len() + 1,
        ChainerError::SegmentCountMismatch
    );
    require!(
        order.route_len()? <= MAX_ROUTE_LEN,
        ChainerError::RouteTooLong
    );

    let order_commitment = order.hash();
    let (escrow_authority, _) = escrow_authority_pda(&order_commitment);

    emit!(OrderAnnounced::new(
        order_commitment,
        escrow_authority,
        order,
    ));

    Ok(())
}
