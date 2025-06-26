use hyper_prover::hyperlane::MAILBOX_ID;
use hyper_prover::instructions::HyperProverError;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn prove_invalid_portal_dispatcher_fail() {
    let mut ctx = common::Context::default();
    let invalid_dispatcher = ctx.payer.insecure_clone();
    let unique_message = Keypair::new();
    let outbox_pda = Pubkey::find_program_address(&[b"hyperlane", b"-", b"outbox"], &MAILBOX_ID).0;
    let dispatched_message_pda = Pubkey::find_program_address(
        &[
            b"hyperlane",
            b"-",
            b"dispatched_message",
            b"-",
            unique_message.pubkey().as_ref(),
        ],
        &MAILBOX_ID,
    )
    .0;

    let result = ctx.hyper_prover_prove(
        &invalid_dispatcher,
        1,
        [0u8; 32].into(),
        vec![0u8; 32],
        [1u8; 32].into(),
        outbox_pda,
        &unique_message,
        dispatched_message_pda,
    );
    assert!(result.is_err_and(common::is_portal_error(
        HyperProverError::InvalidPortalDispatcher
    )));
}
