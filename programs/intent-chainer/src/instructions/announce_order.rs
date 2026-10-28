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
    /// announced separately — by [`announce_template`] when it travels inline, or
    /// by `seal_order_buffer` when it is staged — because one template serves many
    /// orders and is the large half.
    pub order: Order,
}

/// Args for [`announce_template`].
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AnnounceTemplateArgs {
    /// The template preimage to put on the record, keyed by its own hash.
    pub template: TemplateProgram,
}

/// Self-CPI event accounts. No custody or new escrow account is introduced.
#[event_cpi]
#[derive(Accounts)]
pub struct AnnounceOrder {}

/// The same self-CPI event accounts as [`AnnounceOrder`], and nothing else.
#[event_cpi]
#[derive(Accounts)]
pub struct AnnounceTemplate {}

/// Records an order's parameters, both commitment halves and the escrow
/// authority they derive as a durable self-CPI event. This is half of the
/// record: recovering a funded escrow also needs the template preimage, which
/// [`announce_template`] or `seal_order_buffer` points to. Publish both before
/// funding intent1 or retain the preimages elsewhere: the escrow address cannot
/// be inverted to recover either half.
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

/// Records a template by hash and length. The template is this instruction's own argument,
/// already on the record in the transaction that carries it, so the event does not copy it a
/// second time.
///
/// Runs the same static validation seal does and keys the record by the same
/// hash `chain` checks `Order::template_hash` against. Permissionless and
/// custody-free: one record serves every order that commits to this template.
pub fn announce_template(ctx: Context<AnnounceTemplate>, args: AnnounceTemplateArgs) -> Result<()> {
    let AnnounceTemplateArgs { template } = args;

    let encoded_len = crate::types::borsh_len(&template)?;
    template.view().validate(encoded_len)?;
    let template_hash = template.hash()?;
    emit_template_announced(
        ctx.accounts.event_authority.to_account_info(),
        template_hash,
        encoded_len as u32,
        None,
    )
}

/// Records a template as a durable self-CPI event: its hash, length and, for a staged one,
/// its buffer, at most 77 bytes of event data whatever the template's size.
pub(crate) fn emit_template_announced(
    authority: AccountInfo,
    template_hash: Bytes32,
    template_len: u32,
    buffer: Option<Pubkey>,
) -> Result<()> {
    let mut data = Vec::with_capacity(8 + TemplateAnnounced::DISCRIMINATOR.len() + 32 + 4 + 33);
    data.extend_from_slice(anchor_lang::event::EVENT_IX_TAG_LE);
    data.extend_from_slice(TemplateAnnounced::DISCRIMINATOR);
    TemplateAnnounced::new(template_hash, template_len, buffer).serialize(&mut data)?;
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
