use anchor_lang::prelude::*;
use eco_svm_std::{account, Bytes32};

use crate::instructions::announce_order::emit_template_sealed;
// Anchor's composite account derive also needs Chain's generated account modules.
use crate::instructions::chain::*;
use crate::instructions::ChainerError;
use crate::state::{OrderBuffer, ORDER_BUFFER_SEED};
use crate::types::{Order, TemplateProgramView, MAX_ORDER_BYTES};

/// Leaves room for init framing, one signature and compute-budget instructions
/// in a legacy 1232-byte packet. Clients must still size their complete envelope.
pub const MAX_ORDER_CHUNK_BYTES: usize = 800;

/// The most an account may grow in one instruction, creation included. A template over
/// this is staged by growing the buffer as its chunks arrive, each growth paid for as
/// it happens; a buffer never reserves more than the template it holds.
pub const MAX_BUFFER_GROWTH: usize =
    anchor_lang::solana_program::entrypoint::MAX_PERMITTED_DATA_INCREASE;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitOrderBufferArgs {
    pub seed: [u8; 32],
    pub template_hash: Bytes32,
    pub template_len: u32,
    pub bytes: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct WriteOrderBufferArgs {
    /// Append-only offset; retries first read the stored `written` cursor.
    pub offset: u32,
    pub bytes: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: InitOrderBufferArgs)]
pub struct InitOrderBuffer<'info> {
    /// Pays all buffer rent and is the only writer/closer. Never forwarded to chain.
    #[account(mut)]
    pub authority: Signer<'info>,
    /// CHECK: PDA checked here, griefing-resistant allocation and header in handler.
    #[account(mut, seeds = [ORDER_BUFFER_SEED, authority.key().as_ref(), &args.seed], bump)]
    pub order_buffer: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct WriteOrderBuffer<'info> {
    /// Pays the rent of each growth of the buffer, as it happens.
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, has_one = authority,
        seeds = [ORDER_BUFFER_SEED, order_buffer.authority.as_ref(), &order_buffer.seed],
        bump = order_buffer.bump)]
    pub order_buffer: Account<'info, OrderBuffer>,
    pub system_program: Program<'info, System>,
}

/// Sealing validates and durably announces the complete preimage in a small
/// instruction. The buffer authority signs only this staging-side operation.
#[event_cpi]
#[derive(Accounts)]
pub struct SealOrderBuffer<'info> {
    pub authority: Signer<'info>,
    #[account(mut, has_one = authority,
        seeds = [ORDER_BUFFER_SEED, order_buffer.authority.as_ref(), &order_buffer.seed],
        bump = order_buffer.bump)]
    pub order_buffer: Account<'info, OrderBuffer>,
}

#[derive(Accounts)]
pub struct ChainFromAccount<'info> {
    /// Read-only, no buffer-authority account or signature. Not forwarded to CPI.
    #[account(seeds = [ORDER_BUFFER_SEED, order_buffer.authority.as_ref(), &order_buffer.seed],
        bump = order_buffer.bump)]
    pub order_buffer: Account<'info, OrderBuffer>,
    /// Exactly the existing chain accounts and constraints, including its rent payer.
    pub chain: Chain<'info>,
}

#[derive(Accounts)]
pub struct CloseOrderBuffer<'info> {
    /// Returns all rent to its original funder, never an execution-tail recipient.
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, has_one = authority, close = authority,
        seeds = [ORDER_BUFFER_SEED, order_buffer.authority.as_ref(), &order_buffer.seed],
        bump = order_buffer.bump)]
    pub order_buffer: Account<'info, OrderBuffer>,
}

pub fn init_order_buffer(ctx: Context<InitOrderBuffer>, args: InitOrderBufferArgs) -> Result<()> {
    require!(
        args.template_len > 0 && args.template_len as usize <= MAX_ORDER_BYTES,
        ChainerError::OrderTooLarge
    );
    require!(
        args.bytes.len() <= MAX_ORDER_CHUNK_BYTES && args.bytes.len() <= args.template_len as usize,
        ChainerError::InvalidOrderBufferWrite
    );
    let authority = ctx.accounts.authority.key();
    let bump = ctx.bumps.order_buffer;
    account::create_account(
        &ctx.accounts.order_buffer,
        &ctx.accounts.authority,
        &ctx.accounts.system_program,
        &crate::ID,
        OrderBuffer::HEADER_LEN
            + (args.template_len as usize).min(MAX_BUFFER_GROWTH - OrderBuffer::HEADER_LEN),
        &[&[ORDER_BUFFER_SEED, authority.as_ref(), &args.seed, &[bump]]],
    )?;
    let header = OrderBuffer {
        authority,
        seed: args.seed,
        template_hash: args.template_hash,
        template_len: args.template_len,
        written: args.bytes.len() as u32,
        sealed: false,
        bump,
    };
    let mut data = ctx.accounts.order_buffer.try_borrow_mut_data()?;
    header.try_serialize(&mut &mut data[..OrderBuffer::HEADER_LEN])?;
    data[OrderBuffer::HEADER_LEN..OrderBuffer::HEADER_LEN + args.bytes.len()]
        .copy_from_slice(&args.bytes);
    Ok(())
}

pub fn write_order_buffer(
    ctx: Context<WriteOrderBuffer>,
    args: WriteOrderBufferArgs,
) -> Result<()> {
    let buffer = &mut ctx.accounts.order_buffer;
    require!(!buffer.sealed, ChainerError::OrderBufferSealed);
    require!(
        args.offset == buffer.written
            && !args.bytes.is_empty()
            && args.bytes.len() <= MAX_ORDER_CHUNK_BYTES
            && (args.offset as usize + args.bytes.len()) <= buffer.template_len as usize,
        ChainerError::InvalidOrderBufferWrite
    );
    let start = OrderBuffer::HEADER_LEN + args.offset as usize;
    let end = start + args.bytes.len();
    let info = buffer.to_account_info();
    if end > info.data_len() {
        grow(
            &info,
            &ctx.accounts.authority,
            &ctx.accounts.system_program,
            (OrderBuffer::HEADER_LEN + buffer.template_len as usize)
                .min(info.data_len() + MAX_BUFFER_GROWTH),
        )?;
    }
    info.try_borrow_mut_data()?[start..end].copy_from_slice(&args.bytes);
    buffer.written += args.bytes.len() as u32;
    Ok(())
}

/// Grow the buffer to `new_len`, topping its rent up from `payer` first. Each call may
/// add at most [`MAX_BUFFER_GROWTH`], the runtime's per-instruction limit.
fn grow<'info>(
    buffer: &AccountInfo<'info>,
    payer: &Signer<'info>,
    system_program: &Program<'info, System>,
    new_len: usize,
) -> Result<()> {
    let shortfall = Rent::get()?
        .minimum_balance(new_len)
        .saturating_sub(buffer.lamports());
    if shortfall > 0 {
        anchor_lang::system_program::transfer(
            CpiContext::new(
                system_program.key(),
                anchor_lang::system_program::Transfer {
                    from: payer.to_account_info(),
                    to: buffer.clone(),
                },
            ),
            shortfall,
        )?;
    }
    buffer.resize(new_len)?;

    Ok(())
}

/// The template's bytes in a complete buffer, borrowed from its account data. Both seal
/// and execute run this, so the header is never treated as attesting arbitrary bytes.
fn template_bytes<'a>(buffer: &Account<OrderBuffer>, data: &'a [u8]) -> Result<&'a [u8]> {
    require!(
        buffer.template_len > 0 && buffer.template_len as usize <= MAX_ORDER_BYTES,
        ChainerError::OrderTooLarge
    );
    require!(
        buffer.written == buffer.template_len,
        ChainerError::OrderBufferIncomplete
    );
    require!(
        data.len() == OrderBuffer::HEADER_LEN + buffer.template_len as usize,
        ChainerError::InvalidBufferedOrder
    );

    Ok(&data[OrderBuffer::HEADER_LEN..])
}

fn parse_template(bytes: &[u8]) -> Result<TemplateProgramView<'_>> {
    TemplateProgramView::parse(bytes).map_err(|_| error!(ChainerError::InvalidBufferedOrder))
}

pub fn seal_order_buffer(ctx: Context<SealOrderBuffer>) -> Result<()> {
    require!(
        !ctx.accounts.order_buffer.sealed,
        ChainerError::OrderBufferSealed
    );
    let info = ctx.accounts.order_buffer.to_account_info();
    let data = info.try_borrow_data()?;
    let bytes = template_bytes(&ctx.accounts.order_buffer, &data)?;
    parse_template(bytes)?.validate(bytes.len())?;
    // The buffer is the contiguous Borsh copy the commitment hashes: no second one.
    let hash: Bytes32 = solana_sha256_hasher::hashv(&[bytes]).to_bytes().into();
    require!(
        hash == ctx.accounts.order_buffer.template_hash,
        ChainerError::TemplateCommitmentMismatch
    );
    emit_template_sealed(
        ctx.accounts.event_authority.to_account_info(),
        hash,
        ctx.accounts.order_buffer.template_len,
        ctx.accounts.order_buffer.key(),
    )?;
    drop(data);
    ctx.accounts.order_buffer.sealed = true;
    Ok(())
}

pub fn chain_from_account<'info>(
    ctx: Context<'info, ChainFromAccount<'info>>,
    order: Order,
    publish: bool,
) -> Result<()> {
    require!(
        ctx.accounts.order_buffer.sealed,
        ChainerError::OrderBufferNotSealed
    );
    let info = ctx.accounts.order_buffer.to_account_info();
    let data = info.try_borrow_data()?;
    let bytes = template_bytes(&ctx.accounts.order_buffer, &data)?;
    // Read in place: the root route's segments are never copied out of the account.
    // Seal already bound these immutable bytes to the header's `template_hash`, so the
    // commitment is the header's, not a second pass over the payload.
    let template = PresentedTemplate {
        view: parse_template(bytes)?,
        hash: ctx.accounts.order_buffer.template_hash,
        encoded_len: bytes.len(),
    };
    // Delegate every custody/template/measurement/settlement check to the same
    // handler, not a CPI (and never forward the buffer authority). That handler
    // checks `order.template_hash` against this template, recomputes both
    // commitment halves and verifies the supplied escrow before any value moves.
    chain_order(
        Context::new(
            ctx.program_id,
            &mut ctx.accounts.chain,
            ctx.remaining_accounts,
            ctx.bumps.chain,
        ),
        order,
        template,
        publish,
    )
}

/// Available in every state, including incomplete, invalid, abandoned or failed
/// orders. Only reclaims transport rent; never touches the token escrow.
pub fn close_order_buffer(_ctx: Context<CloseOrderBuffer>) -> Result<()> {
    Ok(())
}
