use cctp_prover::cctp::{self, SOLANA_DOMAIN};
use eco_svm_std::prover::ProofData;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{
    attest, evm_sender, format_message, install_transmitter, peers, TestAttester, BASE_CHAIN_ID,
    BASE_DOMAIN, TEST_ATTESTER_SECRET,
};

pub mod common;

fn message_to_cctp_prover(nonce: [u8; 32]) -> Vec<u8> {
    format_message(
        BASE_DOMAIN,
        SOLANA_DOMAIN,
        nonce,
        &Pubkey::new_from_array(evm_sender(0xba)),
        &cctp_prover::ID,
        1000,
        2000,
        &ProofData::new(BASE_CHAIN_ID, vec![]).to_bytes(),
    )
}

fn initialized() -> common::Context {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.cctp_prover().install(authority.pubkey());
    context.cctp_prover().init(&authority, peers()).unwrap();
    context
}

/// litesvm runs Circle's binary: a correctly attested message passes the
/// signature, domain and recipient checks, Circle CPIs our handler, and the
/// batch (empty body, 0 pairs) is a valid no-op, which proves the full path.
#[test]
fn attested_message_is_received_end_to_end() {
    let mut context = initialized();
    let attester = TestAttester::new(TEST_ATTESTER_SECRET);
    let message = message_to_cctp_prover([1; 32]);
    let attestation = attest(std::slice::from_ref(&attester), &message);
    let payer = context.payer.pubkey();
    let ix = cctp::receive_message_instruction(&payer, message, attestation, &[]).unwrap();

    let result = context.cctp_prover().send(vec![ix], &[]);

    assert!(result.is_ok());
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
    let mut context = initialized();
    let attesters = [TestAttester::new([0x31; 32]), TestAttester::new([0x32; 32])];
    install_transmitter(&mut context, &attesters, 2);
    let message = message_to_cctp_prover([3; 32]);
    let attestation = attest(&attesters, &message);
    assert_eq!(attestation.len(), 130);
    let payer = context.payer.pubkey();
    let ix = cctp::receive_message_instruction(&payer, message, attestation, &[]).unwrap();

    let result = context.cctp_prover().send(vec![ix], &[]);

    assert!(result.is_ok());
}

/// Circle requires strictly ascending attester addresses; a reversed pair is
/// rejected even though both signatures are valid.
#[test]
fn reversed_signature_order_is_rejected_by_transmitter() {
    let mut context = initialized();
    let attesters = [TestAttester::new([0x31; 32]), TestAttester::new([0x32; 32])];
    install_transmitter(&mut context, &attesters, 2);
    let message = message_to_cctp_prover([4; 32]);
    let mut ordered: Vec<_> = attesters.iter().collect();
    ordered.sort_by_key(|attester| attester.address());
    let reversed: Vec<u8> = ordered
        .iter()
        .rev()
        .flat_map(|attester| attester.sign(&message))
        .collect();
    let payer = context.payer.pubkey();
    let ix = cctp::receive_message_instruction(&payer, message, reversed, &[]).unwrap();

    let failure = context.cctp_prover().send(vec![ix], &[]).unwrap_err();

    assert!(failure
        .meta
        .logs
        .iter()
        .any(|log| log.contains("InvalidSignatureOrderOrDupe")));
}

/// A valid attestation does not cover a message altered after signing.
#[test]
fn tampered_message_is_rejected_by_transmitter() {
    let mut context = initialized();
    let attester = TestAttester::new(TEST_ATTESTER_SECRET);
    let message = message_to_cctp_prover([5; 32]);
    let attestation = attest(std::slice::from_ref(&attester), &message);
    let mut tampered = message;
    *tampered.last_mut().unwrap() ^= 0x01;
    let payer = context.payer.pubkey();
    let ix = cctp::receive_message_instruction(&payer, tampered, attestation, &[]).unwrap();

    let failure = context.cctp_prover().send(vec![ix], &[]).unwrap_err();

    assert!(failure
        .meta
        .logs
        .iter()
        .any(|log| log.contains("InvalidAttesterSignature")));
}
