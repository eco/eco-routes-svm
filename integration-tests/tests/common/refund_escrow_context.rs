//! Refund clients use only the committed order and existing token accounts.
//! Transactions are submitted as real, packet-sized native instructions.

use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use intent_chainer::instructions::RefundEscrowArgs;
use intent_chainer::state::escrow_authority_pda;
use intent_chainer::types::Order;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use super::template_transport::{transaction_size, PACKET_BYTES};
use super::{order_buffer_context, Context, TransactionResult};

pub fn escrow_accounts(ctx: &Context, order: &Order) -> (Pubkey, Pubkey) {
    let authority = escrow_authority_pda(&order.hash()).0;
    let ata = get_associated_token_address_with_program_id(
        &authority,
        &order.base_mint,
        &ctx.token_program,
    );
    (authority, ata)
}

pub fn inline(ctx: &Context, order: &Order, recipient: Pubkey) -> Instruction {
    let (escrow_authority, escrow_ata) = escrow_accounts(ctx, order);
    Instruction {
        program_id: intent_chainer::ID,
        accounts: intent_chainer::accounts::RefundEscrow {
            escrow_authority,
            escrow_ata,
            base_mint: order.base_mint,
            refund_token_account: recipient,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            creator: order.reward.creator,
        }
        .to_account_metas(None),
        data: intent_chainer::instruction::RefundEscrow {
            args: RefundEscrowArgs {
                order: order.clone(),
            },
        }
        .data(),
    }
}

pub fn buffered(ctx: &Context, order: &Order, recipient: Pubkey, buffer: Pubkey) -> Instruction {
    let mut instruction = inline(ctx, order, recipient);
    instruction
        .accounts
        .insert(0, AccountMeta::new_readonly(buffer, false));
    instruction.data = intent_chainer::instruction::RefundEscrowFromAccount {}.data();
    instruction
}

pub fn send(ctx: &mut Context, instruction: Instruction) -> TransactionResult {
    // The fee payer is neither the refund recipient nor a buffer authority.
    let payer = ctx.payer.insecure_clone();
    order_buffer_context::send(ctx, &payer, &[instruction]).0
}

pub fn refund(ctx: &mut Context, order: &Order, recipient: Pubkey) -> TransactionResult {
    let instruction = inline(ctx, order, recipient);
    send(ctx, instruction)
}

pub fn send_with_creator(
    ctx: &mut Context,
    mut instruction: Instruction,
    creator: &Keypair,
) -> TransactionResult {
    instruction.accounts.last_mut().unwrap().is_signer = true;
    let payer = ctx.payer.insecure_clone();
    let instructions = [
        ComputeBudgetInstruction::set_compute_unit_limit(ctx.compute_limit),
        ComputeBudgetInstruction::set_compute_unit_price(1),
        instruction,
    ];
    let transaction = Transaction::new(
        &[&payer, creator],
        Message::new(&instructions, Some(&payer.pubkey())),
        ctx.latest_blockhash(),
    );
    assert!(transaction_size(&transaction) <= PACKET_BYTES);
    ctx.send_transaction(transaction)
}
