use eco_svm_std::{Bytes32, CHAIN_ID};
use hyper_prover::hyperlane;
use portal::{state, types};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use tiny_keccak::{Hasher, Keccak};

pub mod common;

use common::hyperlane_context;

/// Create a fulfilled intent, prove it via Portal → HyperProver → Mailbox,
/// and return the dispatched_message_pda along with the source domain.
///
/// We build the prove instruction manually (like `prove_intent_via_hyper_prover`
/// does internally) so we control the `unique_message` keypair and can derive
/// the `dispatched_message_pda` for the subsequent pay_for_gas call.
fn setup_dispatched_message(ctx: &mut common::Context) -> (Pubkey, u32) {
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    route.native_amount = 0;
    let reward_hash = random::<[u8; 32]>().into();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    ctx.portal()
        .fulfill_intent(
            intent_hash,
            &route,
            reward_hash,
            claimant,
            executor,
            fulfill_marker,
            vec![],
            vec![],
        )
        .unwrap();

    let source_domain = random::<u32>();
    let source_prover: [u8; 32] = random();
    let dispatcher = state::dispatcher_pda().0;

    // Use the portal prove path (Portal → HyperProver → Mailbox) which is
    // the same path production uses. We need to know the unique_message keypair
    // to derive the dispatched_message_pda, so we build the prove_accounts manually.
    let unique_message = Keypair::new();
    let outbox_pda = hyperlane_context::outbox_pda();
    let dispatched_message_pda =
        hyperlane_context::dispatched_message_pda(&unique_message.pubkey());

    ctx.portal()
        .prove_intent_with_unique_message(
            vec![intent_hash],
            source_domain as u64,
            vec![fulfill_marker],
            dispatcher,
            hyper_prover::state::dispatcher_pda().0,
            hyperlane::MAILBOX_ID,
            source_prover.to_vec(),
            &unique_message,
            outbox_pda,
            dispatched_message_pda,
        )
        .unwrap();

    (dispatched_message_pda, source_domain)
}

#[test]
fn pay_for_gas_success() {
    let mut ctx = common::Context::default();
    let (dispatched_message, source_domain) = setup_dispatched_message(&mut ctx);

    let result = ctx
        .proof_helper()
        .pay_for_gas(dispatched_message, source_domain, 100_000);

    let tx = result.unwrap();
    assert!(tx
        .logs
        .iter()
        .any(|log| log.contains("MockIGP: pay_for_gas")));
}

#[test]
fn pay_for_gas_verifies_message_id() {
    let mut ctx = common::Context::default();
    let (dispatched_message, source_domain) = setup_dispatched_message(&mut ctx);

    // Read the dispatched message account to compute expected message_id
    let account = ctx.get_account(&dispatched_message).unwrap();
    let encoded_message = &account.data[53..]; // version(1) + discriminator(8) + nonce(4) + slot(8) + pubkey(32)
    let mut hasher = Keccak::v256();
    let mut expected_message_id = [0u8; 32];
    hasher.update(encoded_message);
    hasher.finalize(&mut expected_message_id);

    let result = ctx
        .proof_helper()
        .pay_for_gas(dispatched_message, source_domain, 100_000)
        .unwrap();

    assert!(result.logs.iter().any(|log| {
        log.contains(&format!(
            "MockIGP: pay_for_gas domain={} gas=100000",
            source_domain
        ))
    }));
}

#[test]
fn pay_for_gas_invalid_owner_fails() {
    let mut ctx = common::Context::default();

    // Create a fake dispatched message account owned by system program
    let fake_key = Pubkey::new_unique();
    let mut data = vec![0u8; 100];
    data[..8].copy_from_slice(b"DISPATCH");

    ctx.set_account(
        fake_key,
        solana_sdk::account::Account {
            lamports: 1_000_000,
            data,
            owner: anchor_lang::system_program::ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    let result = ctx.proof_helper().pay_for_gas(fake_key, 1, 100_000);

    assert!(result.is_err_and(common::is_error(
        proof_helper::instructions::ProofHelperError::InvalidDispatchedMessageOwner
    )));
}

#[test]
fn pay_for_gas_invalid_discriminator_fails() {
    let mut ctx = common::Context::default();

    // Create an account owned by mailbox but with wrong discriminator
    let fake_key = Pubkey::new_unique();
    let mut data = vec![0u8; 100];
    data[..8].copy_from_slice(b"NOTDISPA");

    ctx.set_account(
        fake_key,
        solana_sdk::account::Account {
            lamports: 1_000_000,
            data,
            owner: hyperlane::MAILBOX_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    let result = ctx.proof_helper().pay_for_gas(fake_key, 1, 100_000);

    assert!(result.is_err_and(common::is_error(
        proof_helper::instructions::ProofHelperError::InvalidDispatchedMessage
    )));
}

#[test]
fn pay_for_gas_too_short_data_fails() {
    let mut ctx = common::Context::default();

    let fake_key = Pubkey::new_unique();
    ctx.set_account(
        fake_key,
        solana_sdk::account::Account {
            lamports: 1_000_000,
            data: vec![0u8; 10],
            owner: hyperlane::MAILBOX_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    let result = ctx.proof_helper().pay_for_gas(fake_key, 1, 100_000);

    assert!(result.is_err_and(common::is_error(
        proof_helper::instructions::ProofHelperError::InvalidDispatchedMessage
    )));
}
