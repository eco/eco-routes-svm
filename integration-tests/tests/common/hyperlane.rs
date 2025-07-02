use anchor_lang::prelude::borsh::{BorshDeserialize, BorshSerialize};
use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::{InstructionData, ToAccountMetas};
use eco_svm_std::CHAIN_ID;
use hyper_prover::hyperlane::{process_authority_pda, MAILBOX_ID, MULTISIG_ISM_MESSAGE_ID};
use litesvm::LiteSVM;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::{sol_amount, Context};

const MAILBOX_BIN: &[u8] = include_bytes!("../../../bins/mailbox.so");
const DUMMY_ISM_BIN: &[u8] = include_bytes!("../../../target/deploy/dummy_ism.so");
const SPL_NOOP_BIN: &[u8] = include_bytes!("../../../bins/noop.so");

pub fn add_hyperlane_programs(svm: &mut LiteSVM) {
    svm.add_program(MAILBOX_ID, MAILBOX_BIN);
    svm.add_program(MULTISIG_ISM_MESSAGE_ID, DUMMY_ISM_BIN);
    svm.add_program(spl_noop::ID, SPL_NOOP_BIN);
}

pub fn init_hyperlane(svm: &mut LiteSVM) {
    let dummy_ism_pda = init_dummy_ism(svm);
    init_mailbox(svm, dummy_ism_pda);
}

#[derive(BorshDeserialize, BorshSerialize)]
enum MailboxInstruction {
    Init(Init),
    InboxProcess(InboxProcess),
    OutboxDispatch(OutboxDispatch),
}

#[derive(BorshDeserialize, BorshSerialize)]
struct InboxProcess {
    pub metadata: Vec<u8>,
    pub message: Vec<u8>,
}

#[derive(BorshDeserialize, BorshSerialize)]
struct OutboxDispatch {
    pub sender: Pubkey,
    pub destination_domain: u32,
    pub recipient: [u8; 32],
    pub message_body: Vec<u8>,
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

fn init_dummy_ism(svm: &mut LiteSVM) -> Pubkey {
    let initializer = Keypair::new();
    svm.airdrop(&initializer.pubkey(), sol_amount(1.0)).unwrap();

    let ism_state_pda = Pubkey::find_program_address(&[b"ism_state"], &MULTISIG_ISM_MESSAGE_ID).0;

    let init_instruction = dummy_ism::instruction::Init {};
    let accounts = dummy_ism::accounts::Init {
        ism_state: ism_state_pda,
        payer: initializer.pubkey(),
        system_program: anchor_lang::system_program::ID,
    };

    let instruction = Instruction {
        program_id: MULTISIG_ISM_MESSAGE_ID,
        accounts: accounts.to_account_metas(None),
        data: init_instruction.data(),
    };

    let transaction = Transaction::new(
        &[&initializer],
        Message::new(&[instruction], Some(&initializer.pubkey())),
        svm.latest_blockhash(),
    );

    svm.send_transaction(transaction).unwrap();

    ism_state_pda
}

fn init_mailbox(svm: &mut LiteSVM, default_ism: Pubkey) {
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
            local_domain: CHAIN_ID.try_into().unwrap(),
            default_ism,
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

impl Context {
    pub fn outbox_dispatch(
        &mut self,
        destination_domain: u32,
        recipient: eco_svm_std::Bytes32,
        message_body: Vec<u8>,
    ) -> crate::common::TransactionResult {
        let payer_pubkey = self.payer.pubkey();
        let outbox_pda = get_outbox_pda();

        // Create a unique message ID
        let unique_message = solana_sdk::signature::Keypair::new();
        let dispatched_message_pda = get_dispatched_message_pda(&unique_message.pubkey());

        // Create the OutboxDispatch instruction
        let instruction_data = OutboxDispatch {
            sender: payer_pubkey,
            destination_domain,
            recipient: *recipient,
            message_body,
        };

        let accounts = vec![
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false), // system_program
            AccountMeta::new(payer_pubkey, true),                              // payer
            AccountMeta::new(outbox_pda, false),                               // outbox
            AccountMeta::new_readonly(spl_noop::ID, false),                    // spl_noop
            AccountMeta::new_readonly(unique_message.pubkey(), true),          // unique_message
            AccountMeta::new(dispatched_message_pda, false),                   // dispatched_message
        ];

        let instruction = Instruction {
            program_id: MAILBOX_ID,
            accounts,
            data: borsh::to_vec(&MailboxInstruction::OutboxDispatch(instruction_data)).unwrap(),
        };

        let transaction = Transaction::new(
            &[&self.payer, &unique_message],
            Message::new(&[instruction], Some(&payer_pubkey)),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }

    pub fn inbox_process(
        &mut self,
        message: Vec<u8>,
        handle_account_metas: Vec<AccountMeta>,
    ) -> crate::common::TransactionResult {
        let inbox_pda =
            Pubkey::find_program_address(&[b"hyperlane", b"-", b"inbox"], &MAILBOX_ID).0;
        let message_hash = solana_sdk::keccak::hash(&message);
        let processed_message_pda = Pubkey::find_program_address(
            &[
                b"hyperlane",
                b"-",
                b"processed_message",
                b"-",
                message_hash.as_ref(),
            ],
            &MAILBOX_ID,
        )
        .0;
        let process_authority_pda = process_authority_pda().0;
        let instruction_data = InboxProcess {
            metadata: vec![],
            message,
        };

        let mut accounts = vec![
            // 0-4: Core mailbox accounts (matching routes-solana exactly)
            AccountMeta::new(self.payer.pubkey(), true),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
            AccountMeta::new(inbox_pda, false),
            AccountMeta::new_readonly(process_authority_pda, false),
            AccountMeta::new(processed_message_pda, false),
        ];
        accounts.extend(
            // 5: ISM account metas
            self.ism_account_metas(),
        );
        accounts.extend(vec![
            // 6: SPL-noop
            AccountMeta::new_readonly(spl_noop::ID, false),
            // 7: ISM program id (dummy ISM)
            AccountMeta::new_readonly(dummy_ism::ID, false),
            // 8: ISM verify accounts (dummy ISM state)
            AccountMeta::new(
                Pubkey::find_program_address(&[b"ism_state"], &dummy_ism::ID).0,
                false,
            ),
            // 9: Recipient program id
            AccountMeta::new_readonly(hyper_prover::ID, false),
        ]);
        // 10+: Handle accounts (matching hyper-prover structure)
        accounts.extend(handle_account_metas);

        let instruction = Instruction {
            program_id: MAILBOX_ID,
            accounts,
            data: borsh::to_vec(&MailboxInstruction::InboxProcess(instruction_data)).unwrap(),
        };
        let transaction = Transaction::new(
            &[&self.payer],
            Message::new(&[instruction], Some(&self.payer.pubkey())),
            self.latest_blockhash(),
        );

        self.send_transaction(transaction)
    }
}
