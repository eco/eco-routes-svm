use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke_signed;
use eco_svm_std::Bytes32;

use crate::events::{OrderAnnounced, TemplateAnnounced};
use crate::state::escrow_authority_pda;
use crate::types::{Order, TemplateProgram};

/// Args for [`announce_order`].
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AnnounceOrderArgs {
    /// The parameters to put on the record. The template this commits to is
    /// announced separately — by `seal_order_buffer`, or by a prior announcement —
    /// because one template serves many orders and is the large half.
    pub order: Order,
}

/// Self-CPI event accounts. No custody or new escrow account is introduced.
#[event_cpi]
#[derive(Accounts)]
pub struct AnnounceOrder {}

/// Records the complete, statically validated preimage as a durable self-CPI
/// event. Publish it before funding intent1 or retain the preimage elsewhere:
/// the escrow address cannot be inverted to recover a lost template.
///
/// Permissionless and custody-free. An announcement does not prove that all
/// future measurements fit the template, attest opaque remote route semantics,
/// or bypass local account checks. See the README for retry limits.
/// Indexers must consume inner instructions, not just ordinary logs.
pub fn announce_order(ctx: Context<AnnounceOrder>, args: AnnounceOrderArgs) -> Result<()> {
    let AnnounceOrderArgs { order } = args;

    order.validate_params()?;
    let params_hash = order.params_hash()?;
    let (escrow_authority, _) = escrow_authority_pda(&order.template_hash, &params_hash);

    let mut data = Vec::with_capacity(
        8 + OrderAnnounced::DISCRIMINATOR.len() + 96 + crate::types::borsh_len(&order)?,
    );
    data.extend_from_slice(anchor_lang::event::EVENT_IX_TAG_LE);
    data.extend_from_slice(OrderAnnounced::DISCRIMINATOR);
    OrderAnnounced::new(order.template_hash, params_hash, escrow_authority, order)
        .serialize(&mut data)?;
    emit_self_cpi(ctx.accounts.event_authority.to_account_info(), data)
}

pub(crate) fn emit_template_announcement(
    authority: AccountInfo,
    template: TemplateProgram,
    template_hash: Bytes32,
) -> Result<()> {

    let mut data = Vec::with_capacity(
        8 + TemplateAnnounced::DISCRIMINATOR.len() + 32 + crate::types::borsh_len(&template)?,
    );
    data.extend_from_slice(anchor_lang::event::EVENT_IX_TAG_LE);
    data.extend_from_slice(TemplateAnnounced::DISCRIMINATOR);
    TemplateAnnounced::new(template_hash, template).serialize(&mut data)?;
    emit_self_cpi(authority, data)
}

/// Wire-identical to `emit_cpi!`, but over ONE pre-sized buffer the caller owns.
/// The macro grows `Event::data()`, prepends the CPI tag into another Vec, then
/// copies it again through `Instruction::new_with_bytes`. Those unreclaimed
/// buffers exhaust the stock heap on dense nested templates. Never fall back to a
/// droppable log.
fn emit_self_cpi(authority: AccountInfo, data: Vec<u8>) -> Result<()> {
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
