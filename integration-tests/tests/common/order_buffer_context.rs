//! Native chainer transport. No generic staging program or oversized submissions.
use anchor_lang::{InstructionData, ToAccountMetas};
use eco_svm_std::Bytes32;
use intent_chainer::instructions::{
    InitOrderBufferArgs, WriteOrderBufferArgs, MAX_ORDER_CHUNK_BYTES,
};
use intent_chainer::state::OrderBuffer;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use super::template_transport::{transaction_size, PACKET_BYTES};
use super::{Context, TransactionResult};

pub fn init(
    authority: Pubkey,
    seed: [u8; 32],
    commitment: Bytes32,
    len: usize,
    bytes: &[u8],
) -> Instruction {
    Instruction {
        program_id: intent_chainer::ID,
        accounts: intent_chainer::accounts::InitOrderBuffer {
            authority,
            order_buffer: OrderBuffer::pda(&authority, &seed).0,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: intent_chainer::instruction::InitOrderBuffer {
            args: InitOrderBufferArgs {
                seed,
                order_commitment: commitment,
                order_len: len.try_into().unwrap(),
                bytes: bytes.to_vec(),
            },
        }
        .data(),
    }
}

pub fn write(authority: Pubkey, buffer: Pubkey, offset: u32, bytes: &[u8]) -> Instruction {
    Instruction {
        program_id: intent_chainer::ID,
        accounts: intent_chainer::accounts::WriteOrderBuffer {
            authority,
            order_buffer: buffer,
        }
        .to_account_metas(None),
        data: intent_chainer::instruction::WriteOrderBuffer {
            args: WriteOrderBufferArgs {
                offset,
                bytes: bytes.to_vec(),
            },
        }
        .data(),
    }
}

pub fn seal(authority: Pubkey, buffer: Pubkey) -> Instruction {
    Instruction {
        program_id: intent_chainer::ID,
        accounts: intent_chainer::accounts::SealOrderBuffer {
            authority,
            order_buffer: buffer,
            event_authority: intent_chainer::EVENT_AUTHORITY_AND_BUMP.0,
            program: intent_chainer::ID,
        }
        .to_account_metas(None),
        data: intent_chainer::instruction::SealOrderBuffer {}.data(),
    }
}

pub fn close(authority: Pubkey, buffer: Pubkey) -> Instruction {
    Instruction {
        program_id: intent_chainer::ID,
        accounts: intent_chainer::accounts::CloseOrderBuffer {
            authority,
            order_buffer: buffer,
        }
        .to_account_metas(None),
        data: intent_chainer::instruction::CloseOrderBuffer {}.data(),
    }
}

/// The IDL account order is [read-only buffer, existing Chain accounts].
pub fn execute(mut chain: Instruction, buffer: Pubkey, publish: bool) -> Instruction {
    chain
        .accounts
        .insert(0, AccountMeta::new_readonly(buffer, false));
    chain.data = intent_chainer::instruction::ChainFromAccount { publish }.data();
    chain
}

/// Legacy packet including a real signature, blockhash, CU limit AND CU price.
pub fn transaction(ctx: &Context, payer: &Keypair, instructions: &[Instruction]) -> Transaction {
    let instructions: Vec<_> = [
        ComputeBudgetInstruction::set_compute_unit_limit(ctx.compute_limit),
        ComputeBudgetInstruction::set_compute_unit_price(1),
    ]
    .into_iter()
    .chain(instructions.iter().cloned())
    .collect();
    Transaction::new(
        &[payer],
        Message::new(&instructions, Some(&payer.pubkey())),
        ctx.latest_blockhash(),
    )
}

pub fn send(
    ctx: &mut Context,
    payer: &Keypair,
    instructions: &[Instruction],
) -> (TransactionResult, usize) {
    // Fresh messages exercise program replay checks, not the runtime signature cache.
    ctx.expire_blockhash();
    let tx = transaction(ctx, payer, instructions);
    let size = transaction_size(&tx);
    assert!(size <= PACKET_BYTES, "native staged packet is {size} bytes");
    let result = ctx.send_transaction(tx);
    if let Ok(meta) = &result {
        println!(
            "native staging: packet={size} CU={}",
            meta.compute_units_consumed
        );
        for log in meta.logs.iter().filter(|l| l.contains("chainer heap")) {
            println!("{log}");
        }
    }
    (result, size)
}

/// Upload all bytes in real bounded transactions. Leaves the buffer unsealed so
/// tests can pair seal/announcement with ATA creation in the second prepare phase.
pub fn upload(
    ctx: &mut Context,
    authority: &Keypair,
    seed: [u8; 32],
    commitment: Bytes32,
    bytes: &[u8],
) -> Pubkey {
    let first = bytes.len().min(MAX_ORDER_CHUNK_BYTES);
    send(
        ctx,
        authority,
        &[init(
            authority.pubkey(),
            seed,
            commitment,
            bytes.len(),
            &bytes[..first],
        )],
    )
    .0
    .unwrap();
    let buffer = OrderBuffer::pda(&authority.pubkey(), &seed).0;
    for (index, chunk) in bytes[first..].chunks(MAX_ORDER_CHUNK_BYTES).enumerate() {
        send(
            ctx,
            authority,
            &[write(
                authority.pubkey(),
                buffer,
                (first + index * MAX_ORDER_CHUNK_BYTES) as u32,
                chunk,
            )],
        )
        .0
        .unwrap();
    }
    buffer
}
