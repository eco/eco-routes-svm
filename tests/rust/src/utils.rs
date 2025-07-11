use crate::{helpers, svm_to_svm_e2e::spl_noop};
use borsh::{BorshDeserialize, BorshSerialize};
use core::include_bytes;
use litesvm::LiteSVM;
use solana_instruction::{AccountMeta, Instruction};
use solana_message::Message;
use solana_sdk::{
    native_token::LAMPORTS_PER_SOL, pubkey::Pubkey, signature::Keypair, signer::Signer,
};
use solana_transaction::Transaction;

const MAILBOX_BIN: &[u8] = include_bytes!("../../../bins/mailbox.so");
const DUMMY_ISM_BIN: &[u8] = include_bytes!("../../../bins/dummy_ism.so");
const ECO_ROUTES_BIN: &[u8] = include_bytes!("../../../target/deploy/eco_routes.so");
const SPL_NOOP_BIN: &[u8] = include_bytes!("../../../bins/noop.so");

#[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
pub enum MailboxInstruction {
    /// Initializes the program.
    Init(Init),
}

#[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
pub struct Init {
    /// The local domain of the Mailbox.
    pub local_domain: u32,
    /// The default ISM.
    pub default_ism: Pubkey,
    /// The maximum protocol fee that can be charged.
    pub max_protocol_fee: u64,
    /// The protocol fee configuration.
    pub protocol_fee: ProtocolFee,
}

#[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
pub struct ProtocolFee {
    /// The current protocol fee, expressed in the lowest denomination.
    pub fee: u64,
    /// The beneficiary of protocol fees.
    pub beneficiary: Pubkey,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug)]
pub enum TestIsmInstruction {
    /// Initializes the program.
    Init,
    /// Sets whether messages should be accepted / verified.
    SetAccept(bool),
}

pub fn init_svm() -> LiteSVM {
    let mut svm = LiteSVM::new();

    svm.airdrop(&Keypair::new().pubkey(), 1).unwrap();

    svm.add_program(portal::ID, &ECO_ROUTES_BIN);
    svm.add_program(hyper_prover::hyperlane::MAILBOX_ID, &MAILBOX_BIN);
    svm.add_program(hyper_prover::hyperlane::MULTISIG_ISM_ID, &DUMMY_ISM_BIN);
    svm.add_program(spl_noop::ID, &SPL_NOOP_BIN);

    let inititializer = Keypair::new();
    helpers::write_account_no_data(&mut svm, inititializer.pubkey(), LAMPORTS_PER_SOL).unwrap();

    svm.send_transaction(Transaction::new(
        &[&inititializer],
        Message::new(
            &[Instruction::new_with_borsh(
                hyper_prover::hyperlane::MULTISIG_ISM_ID,
                &TestIsmInstruction::Init,
                vec![
                    AccountMeta::new_readonly(solana_system_interface::program::ID, false),
                    AccountMeta::new(inititializer.pubkey(), true),
                    AccountMeta::new(
                        Pubkey::find_program_address(
                            &[b"test_ism", b"-", b"storage"],
                            &hyper_prover::hyperlane::MULTISIG_ISM_ID,
                        )
                        .0,
                        false,
                    ),
                ],
            )],
            Some(&inititializer.pubkey()),
        ),
        svm.latest_blockhash(),
    ))
    .unwrap();

    svm.send_transaction(Transaction::new(
        &[&inititializer],
        Message::new(
            &[Instruction::new_with_borsh(
                hyper_prover::hyperlane::MAILBOX_ID,
                &MailboxInstruction::Init(Init {
                            local_domain: hyper_prover::hyperlane::DOMAIN_ID,
        default_ism: hyper_prover::hyperlane::MULTISIG_ISM_ID,
                    max_protocol_fee: 0,
                    protocol_fee: ProtocolFee {
                        fee: 0,
                        beneficiary: Pubkey::default(),
                    },
                }),
                vec![
                    AccountMeta::new_readonly(solana_system_interface::program::ID, false),
                    AccountMeta::new(inititializer.pubkey(), true),
                    AccountMeta::new(
                        Pubkey::find_program_address(
                            &[b"hyperlane", b"-", b"inbox"],
                            &hyper_prover::hyperlane::MAILBOX_ID,
                        )
                        .0,
                        false,
                    ),
                    AccountMeta::new(
                        Pubkey::find_program_address(
                            &[b"hyperlane", b"-", b"outbox"],
                            &hyper_prover::hyperlane::MAILBOX_ID,
                        )
                        .0,
                        false,
                    ),
                ],
            )],
            Some(&inititializer.pubkey()),
        ),
        svm.latest_blockhash(),
    ))
    .unwrap();

    svm
}
