use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::Bytes32;

use crate::events::OrderAnnounced;
use crate::instructions::chain::validate_order;
use crate::state::escrow_authority_pda;
use crate::types::Order;

/// Args for [`announce_order`].
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AnnounceOrderArgs {
    /// The order to put on the record.
    pub order: Order,
}

/// Self-CPI event accounts. No custody or new escrow account is introduced.
#[event_cpi]
#[derive(Accounts)]
pub struct AnnounceOrder {}

/// Records the complete, statically validated preimage as a durable self-CPI
/// event. Publish it before funding intent1 or retain the preimage elsewhere:
/// the escrow address cannot be inverted to recover a lost Order.
///
/// Permissionless and custody-free. An announcement does not prove that all
/// future measurements fit the template, attest opaque remote route semantics,
/// or bypass local settlement/collision checks. See the README for retry limits.
/// Indexers must consume inner instructions, not just ordinary logs.
pub fn announce_order(ctx: Context<AnnounceOrder>, args: AnnounceOrderArgs) -> Result<()> {
    let AnnounceOrderArgs { order } = args;

    // Validate the complete nested preimage before hashing or emitting it.
    validate_order(&order)?;

    let order_commitment = order.hash();
    emit_order_announcement(
        ctx.accounts.event_authority.to_account_info(),
        order,
        order_commitment,
    )
}

pub(crate) fn emit_order_announcement(
    authority: AccountInfo,
    order: Order,
    order_commitment: Bytes32,
) -> Result<()> {
    let (escrow_authority, _) = escrow_authority_pda(&order_commitment);

    // Wire-identical to emit_cpi!, but ONE pre-sized buffer. The macro grows
    // Event::data(), prepends the CPI tag into another Vec, then copies it again
    // through Instruction::new_with_bytes. Those unreclaimed buffers exhaust the
    // stock heap on dense nested orders. Never fall back to a droppable log.
    let mut data =
        Vec::with_capacity(8 + OrderAnnounced::DISCRIMINATOR.len() + 64 + order.encoded_len()?);
    data.extend_from_slice(anchor_lang::event::EVENT_IX_TAG_LE);
    data.extend_from_slice(OrderAnnounced::DISCRIMINATOR);
    OrderAnnounced::new(order_commitment, escrow_authority, order).serialize(&mut data)?;
    let instruction = Instruction {
        program_id: crate::ID,
        accounts: vec![AccountMeta::new_readonly(authority.key(), true)],
        data,
    };
    invoke_signed(
        &instruction,
        &[authority],
        &[&[b"__event_authority", &[crate::EVENT_AUTHORITY_AND_BUMP.1]]],
    )?;

    #[cfg(all(feature = "resource-metrics", target_os = "solana"))]
    crate::log_heap_usage("announced");

    Ok(())
}
