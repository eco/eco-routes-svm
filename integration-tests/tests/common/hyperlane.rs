use anchor_lang::prelude::borsh::{BorshDeserialize, BorshSerialize};
use anchor_lang::prelude::{borsh, AccountMeta};
use hyper_prover::hyperlane::MAILBOX_ID;
use litesvm::LiteSVM;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::sol_amount;

const MAILBOX_BIN: &[u8] = include_bytes!("../../../bins/mailbox.so");

pub fn add_hyperlane_programs(svm: &mut LiteSVM) {
    svm.add_program(MAILBOX_ID, MAILBOX_BIN);
}

pub fn init_hyperlane(svm: &mut LiteSVM) {
    init_mailbox(svm);
}

#[derive(BorshDeserialize, BorshSerialize)]
enum MailboxInstruction {
    Init(Init),
}

#[derive(BorshDeserialize, BorshSerialize)]
struct Init {
    pub local_domain: u32,
    pub default_ism: Pubkey,
    pub max_protocol_fee: u64,
    pub protocol_fee: ProtocolFee,
}

#[derive(BorshDeserialize, BorshSerialize)]
struct ProtocolFee {
    pub fee: u64,
    pub beneficiary: Pubkey,
}

fn init_mailbox(svm: &mut LiteSVM) {
    let initializer = Keypair::new();
    svm.airdrop(&initializer.pubkey(), sol_amount(1.0)).unwrap();

    let inbox_pda = Pubkey::find_program_address(&[b"hyperlane", b"-", b"inbox"], &MAILBOX_ID).0;
    let outbox_pda = Pubkey::find_program_address(&[b"hyperlane", b"-", b"outbox"], &MAILBOX_ID).0;

    let init_instruction = Instruction {
        program_id: MAILBOX_ID,
        accounts: vec![
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
            AccountMeta::new(initializer.pubkey(), true),
            AccountMeta::new(inbox_pda, false),
            AccountMeta::new(outbox_pda, false),
        ],
        data: borsh::to_vec(&MailboxInstruction::Init(Init {
            local_domain: 1,
            default_ism: Pubkey::default(),
            max_protocol_fee: 0,
            protocol_fee: ProtocolFee {
                fee: 0,
                beneficiary: Pubkey::default(),
            },
        }))
        .unwrap(),
    };

    let transaction = Transaction::new(
        &[&initializer],
        Message::new(&[init_instruction], Some(&initializer.pubkey())),
        svm.latest_blockhash(),
    );

    svm.send_transaction(transaction).unwrap();
}

pub fn get_outbox_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"hyperlane", b"-", b"outbox"], &MAILBOX_ID).0
}

pub fn get_dispatched_message_pda(unique_message: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"hyperlane",
            b"-",
            b"dispatched_message",
            b"-",
            unique_message.as_ref(),
        ],
        &MAILBOX_ID,
    )
    .0
}
