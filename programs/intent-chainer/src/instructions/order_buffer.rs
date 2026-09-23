use anchor_lang::prelude::*;
use eco_svm_std::{account, Bytes32};

use crate::instructions::announce_order::emit_order_announcement;
// Anchor's composite derives also need the generated nested account modules.
use crate::instructions::chain::*;
use crate::instructions::refund_escrow::*;
use crate::instructions::ChainerError;
use crate::state::{OrderBuffer, ORDER_BUFFER_SEED};
use crate::types::{Order, MAX_ORDER_BYTES};

/// Leaves room for init framing, one signature and compute-budget instructions
/// in a legacy 1232-byte packet. Clients must still size their complete envelope.
pub const MAX_ORDER_CHUNK_BYTES: usize = 800;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitOrderBufferArgs {
    pub seed: [u8; 32],
    pub order_commitment: Bytes32,
    pub order_len: u32,
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
    pub authority: Signer<'info>,
    #[account(mut, has_one = authority,
        seeds = [ORDER_BUFFER_SEED, order_buffer.authority.as_ref(), &order_buffer.seed],
        bump = order_buffer.bump)]
    pub order_buffer: Account<'info, OrderBuffer>,
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
pub struct RefundEscrowFromAccount<'info> {
    /// Complete bytes are sufficient: a continuation that cannot seal must still
    /// be refundable. The actual canonical order hash is checked by the handler.
    #[account(seeds = [ORDER_BUFFER_SEED, order_buffer.authority.as_ref(), &order_buffer.seed],
        bump = order_buffer.bump)]
    pub order_buffer: Account<'info, OrderBuffer>,
    pub refund: RefundEscrow<'info>,
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
        args.order_len > 0 && args.order_len as usize <= MAX_ORDER_BYTES,
        ChainerError::OrderTooLarge
    );
    require!(
        args.bytes.len() <= MAX_ORDER_CHUNK_BYTES && args.bytes.len() <= args.order_len as usize,
        ChainerError::InvalidOrderBufferWrite
    );
    let authority = ctx.accounts.authority.key();
    let bump = ctx.bumps.order_buffer;
    account::create_account(
        &ctx.accounts.order_buffer,
        &ctx.accounts.authority,
        &ctx.accounts.system_program,
        &crate::ID,
        OrderBuffer::HEADER_LEN + args.order_len as usize,
        &[&[ORDER_BUFFER_SEED, authority.as_ref(), &args.seed, &[bump]]],
    )?;
    let header = OrderBuffer {
        authority,
        seed: args.seed,
        order_commitment: args.order_commitment,
        order_len: args.order_len,
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
            && (args.offset as usize + args.bytes.len()) <= buffer.order_len as usize,
        ChainerError::InvalidOrderBufferWrite
    );
    let start = OrderBuffer::HEADER_LEN + args.offset as usize;
    buffer.to_account_info().try_borrow_mut_data()?[start..start + args.bytes.len()]
        .copy_from_slice(&args.bytes);
    buffer.written += args.bytes.len() as u32;
    Ok(())
}

/// Parse exactly one Order directly from borrowed account data. Both seal and
/// execute run this, so the header is never treated as attesting arbitrary bytes.
fn read_order(buffer: &Account<OrderBuffer>) -> Result<Order> {
    require!(
        buffer.order_len > 0 && buffer.order_len as usize <= MAX_ORDER_BYTES,
        ChainerError::OrderTooLarge
    );
    require!(
        buffer.written == buffer.order_len,
        ChainerError::OrderBufferIncomplete
    );
    let info = buffer.to_account_info();
    let data = info.try_borrow_data()?;
    require!(
        data.len() == OrderBuffer::HEADER_LEN + buffer.order_len as usize,
        ChainerError::InvalidBufferedOrder
    );
    Order::try_from_slice(&data[OrderBuffer::HEADER_LEN..])
        .map_err(|_| error!(ChainerError::InvalidBufferedOrder))
}

pub fn seal_order_buffer(ctx: Context<SealOrderBuffer>) -> Result<()> {
    require!(
        !ctx.accounts.order_buffer.sealed,
        ChainerError::OrderBufferSealed
    );
    let order = read_order(&ctx.accounts.order_buffer)?;
    validate_order(&order)?;
    let commitment = order.hash();
    require!(
        commitment == ctx.accounts.order_buffer.order_commitment,
        ChainerError::OrderCommitmentMismatch
    );
    emit_order_announcement(
        ctx.accounts.event_authority.to_account_info(),
        order,
        commitment,
    )?;
    ctx.accounts.order_buffer.sealed = true;
    Ok(())
}

pub fn chain_from_account<'info>(
    ctx: Context<'info, ChainFromAccount<'info>>,
    publish: bool,
) -> Result<()> {
    require!(
        ctx.accounts.order_buffer.sealed,
        ChainerError::OrderBufferNotSealed
    );
    let order = read_order(&ctx.accounts.order_buffer)?;
    // Delegate every custody/template/measurement/settlement check to the same
    // handler, not a CPI (and never forward the buffer authority). That handler
    // recomputes Order.hash() and verifies the supplied escrow before any value
    // moves. Seal has already bound the immutable bytes to the header commitment.
    chain_order(
        Context::new(
            ctx.program_id,
            &mut ctx.accounts.chain,
            ctx.remaining_accounts,
            ctx.bumps.chain,
        ),
        ChainArgs { order, publish },
        Some(ctx.accounts.order_buffer.order_commitment),
    )
}

/// Refund from complete bounded transport bytes, with no writer signature or
/// sealing requirement. Every custody check is shared with the inline entrypoint.
pub fn refund_escrow_from_account<'info>(
    ctx: Context<'info, RefundEscrowFromAccount<'info>>,
) -> Result<()> {
    let order = read_order(&ctx.accounts.order_buffer)?;
    require!(
        order.hash() == ctx.accounts.order_buffer.order_commitment,
        ChainerError::OrderCommitmentMismatch
    );
    refund_escrow(
        Context::new(
            ctx.program_id,
            &mut ctx.accounts.refund,
            ctx.remaining_accounts,
            ctx.bumps.refund,
        ),
        RefundEscrowArgs { order },
    )
}

/// Available in every state, including incomplete, invalid, abandoned or failed
/// orders. Only reclaims transport rent; never touches the token escrow.
pub fn close_order_buffer(_ctx: Context<CloseOrderBuffer>) -> Result<()> {
    Ok(())
}
