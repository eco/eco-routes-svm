use cctp_prover::cctp::{self, SOLANA_DOMAIN};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{
    attest, install_transmitter, TestAttester, BASE_DOMAIN, TEST_ATTESTER_SECRET,
};

pub mod common;

fn message_to_cctp_prover(nonce: [u8; 32]) -> Vec<u8> {
    let mut sender = [0u8; 32];
    sender[12..].fill(0xba);
    cctp::format_message(
        BASE_DOMAIN,
        SOLANA_DOMAIN,
        nonce,
        &Pubkey::new_from_array(sender),
        &cctp_prover::ID,
        1000,
        2000,
        &[],
    )
}

/// litesvm runs Circle's binary: a correctly attested message passes the
/// signature, domain and recipient checks and reaches our program.
/// `handle_receive_*` do not exist until Task 6, so Anchor's fallback rejects
/// the CPI (101 = InstructionFallbackNotFound). Task 6 replaces this test.
#[test]
fn attested_message_reaches_cctp_prover() {
    let mut context = common::Context::default();
    let attester = TestAttester::new(TEST_ATTESTER_SECRET);
    install_transmitter(&mut context, std::slice::from_ref(&attester), 1);
    let message = message_to_cctp_prover([1; 32]);
    let attestation = attest(std::slice::from_ref(&attester), &message);
    let payer = context.payer.pubkey();
    let ix = cctp::receive_message_instruction(&payer, message, attestation, &[]).unwrap();

    let result = context.cctp_prover().send(vec![ix], &[]);

    assert!(result.is_err_and(common::is_error(
        anchor_lang::error::ErrorCode::InstructionFallbackNotFound
    )));
}

#[test]
fn foreign_attester_is_rejected_by_transmitter() {
    let mut context = common::Context::default();
    install_transmitter(&mut context, &[TestAttester::new(TEST_ATTESTER_SECRET)], 1);
    let message = message_to_cctp_prover([2; 32]);
    let attestation = attest(&[TestAttester::new([0x22; 32])], &message);
    let payer = context.payer.pubkey();
    let ix = cctp::receive_message_instruction(&payer, message, attestation, &[]).unwrap();

    let failure = context.cctp_prover().send(vec![ix], &[]).unwrap_err();

    assert!(failure
        .meta
        .logs
        .iter()
        .any(|log| log.contains("InvalidAttesterSignature")));
}

#[test]
fn signatures_are_low_s_and_two_of_two_verifies() {
    let mut context = common::Context::default();
    let attesters = [TestAttester::new([0x31; 32]), TestAttester::new([0x32; 32])];
    install_transmitter(&mut context, &attesters, 2);
    let message = message_to_cctp_prover([3; 32]);
    let attestation = attest(&attesters, &message);
    assert_eq!(attestation.len(), 130);
    let payer = context.payer.pubkey();
    let ix = cctp::receive_message_instruction(&payer, message, attestation, &[]).unwrap();

    let result = context.cctp_prover().send(vec![ix], &[]);

    assert!(result.is_err_and(common::is_error(
        anchor_lang::error::ErrorCode::InstructionFallbackNotFound
    )));
}
