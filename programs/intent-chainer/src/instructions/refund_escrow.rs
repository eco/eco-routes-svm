use anchor_lang::prelude::*;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::{token, token_2022};
use portal::types::TokenTransferAccounts;

use crate::events::EscrowRefunded;
use crate::instructions::ChainerError;
use crate::state::{escrow_authority_pda, ESCROW_SEED};
use crate::types::Order;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RefundEscrowArgs {
    pub order: Order,
}

/// Permissionless: the transaction's payer need not be the refund recipient.
/// The recipient token account must already exist and belong to the committed
/// creator. Permissionless callers must use its canonical ATA; the creator may
/// sign to select another owned account if that ATA cannot receive funds.
#[derive(Accounts)]
pub struct RefundEscrow<'info> {
    /// CHECK: derived from the complete order commitment in the handler.
    pub escrow_authority: UncheckedAccount<'info>,
    /// CHECK: canonical escrow ATA, mint, token program and authority validated below.
    #[account(mut)]
    pub escrow_ata: UncheckedAccount<'info>,
    /// CHECK: must equal the committed base mint and belong to a supported token program.
    pub base_mint: UncheckedAccount<'info>,
    /// CHECK: token program, mint and owner are validated against the committed order.
    #[account(mut)]
    pub refund_token_account: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    /// CHECK: committed creator, optionally signing to authorize a non-ATA recipient.
    pub creator: UncheckedAccount<'info>,
}

/// Recover only the balance still in the intermediate escrow after its committed
/// reward deadline. This does not touch a child vault or its proof/settlement state.
///
/// Deliberately does NOT call validate_order, render a template, enforce the input
/// floor, or invoke Portal. A broken continuation must not disable the custody exit.
/// Order deserialization remains bounded and its complete canonical bytes authorize
/// the escrow and recipient. Inline and buffered transports share this handler.
pub fn refund_escrow(ctx: Context<RefundEscrow>, args: RefundEscrowArgs) -> Result<()> {
    let order = args.order;
    let now = Clock::get()?.unix_timestamp.max(0) as u64;
    require!(
        now >= order.reward.deadline,
        ChainerError::RefundNotAvailable
    );

    let commitment = order.hash();
    let (authority, bump) = escrow_authority_pda(&commitment);
    require_keys_eq!(
        ctx.accounts.escrow_authority.key(),
        authority,
        ChainerError::InvalidEscrowAuthority
    );
    require_keys_eq!(
        ctx.accounts.base_mint.key(),
        order.base_mint,
        ChainerError::InvalidMint
    );
    let expected_ata = get_associated_token_address_with_program_id(
        &authority,
        &order.base_mint,
        ctx.accounts.base_mint.owner,
    );
    require_keys_eq!(
        ctx.accounts.escrow_ata.key(),
        expected_ata,
        ChainerError::InvalidEscrowAta
    );

    let accounts: TokenTransferAccounts = vec![
        &ctx.accounts.escrow_ata.to_account_info(),
        &ctx.accounts.refund_token_account.to_account_info(),
        &ctx.accounts.base_mint.to_account_info(),
    ]
    .try_into()?;
    let token_program = accounts.token_program(
        &ctx.accounts.token_program,
        &ctx.accounts.token_2022_program,
    )?;
    let source = accounts.from_data()?;
    let recipient = accounts.to_data()?;
    require_keys_eq!(
        source.owner,
        authority,
        ChainerError::InvalidEscrowTokenOwner
    );
    require_keys_eq!(source.mint, order.base_mint, ChainerError::InvalidMint);
    require_keys_eq!(recipient.mint, order.base_mint, ChainerError::InvalidMint);
    require_keys_eq!(
        recipient.owner,
        order.reward.creator,
        ChainerError::InvalidRefundRecipient
    );
    require_keys_eq!(
        ctx.accounts.creator.key(),
        order.reward.creator,
        ChainerError::InvalidRefundRecipient
    );
    let creator_ata = get_associated_token_address_with_program_id(
        &order.reward.creator,
        &order.base_mint,
        accounts.token_program_id(),
    );
    require!(
        ctx.accounts.refund_token_account.key() == creator_ata || ctx.accounts.creator.is_signer,
        ChainerError::CreatorSignatureRequired
    );

    // Keep the ATA open: repeats are harmless and later deposits remain recoverable.
    // Do not require exact recipient credit: a mint's transfer fee must not create
    // another escape-path deadlock. Report debit and net credit separately instead.
    let amount = source.amount;
    let signer_seeds = [ESCROW_SEED, commitment.as_ref(), &[bump]];
    accounts.transfer_with_signer(
        &token_program,
        &ctx.accounts.escrow_authority,
        &[&signer_seeds],
        amount,
    )?;
    let amount_received = accounts
        .to_data()?
        .amount
        .checked_sub(recipient.amount)
        .ok_or(ChainerError::RefundBalanceMismatch)?;
    require!(
        accounts.from_data()?.amount == 0 && amount_received <= amount,
        ChainerError::RefundBalanceMismatch
    );

    emit!(EscrowRefunded {
        order_commitment: commitment,
        escrow_authority: authority,
        base_mint: order.base_mint,
        creator: order.reward.creator,
        amount,
        amount_received,
    });
    Ok(())
}
