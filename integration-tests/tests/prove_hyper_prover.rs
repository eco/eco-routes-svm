use std::vec;

use anchor_lang::error::ErrorCode;
use anchor_lang::AnchorDeserialize;
use eco_svm_std::{Bytes32, CHAIN_ID};
use hyper_prover::hyperlane::{self, MailboxInstruction, MAILBOX_ID};
use hyper_prover::instructions::HyperProverError;
use portal::events::IntentProven;
use portal::{state, types};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

pub mod common;

fn setup() -> (common::Context, Bytes32) {
    let mut ctx = common::Context::default();
    let mut route = ctx.rand_intent().route;
    route.tokens.clear();
    route.calls.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    ctx.portal()
        .fulfill_intent(
            &route,
            reward_hash,
            claimant,
            executor,
            fulfill_marker,
            vec![],
            vec![],
        )
        .unwrap();

    (ctx, intent_hash)
}

#[test]
fn prove_intent_success() {
    let (mut ctx, intent_hash) = setup();
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source_chain = random::<u32>() as u64;
    let source_chain_prover = random::<[u8; 32]>();
    let claimant = ctx
        .account::<state::FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hash,
        source_chain,
        fulfill_marker,
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        source_chain_prover.into(),
    );
    assert!(result.clone().is_ok_and(common::contains_event_and_msg(
        IntentProven::new(intent_hash, source_chain, CHAIN_ID),
        "Dispatched message"
    )));

    let tx = result.unwrap();
    let mailbox_dispatch: Vec<_> = tx
        .inner_instructions
        .into_iter()
        .flat_map(IntoIterator::into_iter)
        .filter_map(|ix| MailboxInstruction::try_from_slice(ix.instruction.data.as_slice()).ok())
        .collect();
    assert_eq!(mailbox_dispatch.len(), 1);

    match mailbox_dispatch.first().unwrap() {
        MailboxInstruction::OutboxDispatch(msg) => {
            assert_eq!(msg.sender, hyper_prover::state::dispatcher_pda().0);
            assert_eq!(msg.destination_domain, source_chain as u32);
            assert_eq!(msg.recipient, source_chain_prover);
            assert_eq!(
                msg.message_body,
                claimant.into_iter().chain(intent_hash).collect::<Vec<_>>()
            );
        }
        _ => panic!("expected OutboxDispatch instruction"),
    }
}

#[test]
fn prove_intent_invalid_dispatcher_fail() {
    let (mut ctx, intent_hash) = setup();
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let invalid_dispatcher = Pubkey::new_unique();
    let source_chain = random::<u32>() as u64;

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hash,
        source_chain,
        fulfill_marker,
        invalid_dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        random::<[u8; 32]>().into(),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidDispatcher
    )));
}

#[test]
fn prove_intent_unfulfilled_fail() {
    let mut ctx = common::Context::default();
    let intent_hash = rand::random::<[u8; 32]>().into();
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source_chain = random::<u32>() as u64;

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hash,
        source_chain,
        fulfill_marker,
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        random::<[u8; 32]>().into(),
    );
    assert!(result.is_err_and(common::is_error(ErrorCode::AccountNotInitialized)));
}

#[test]
fn prove_intent_invalid_hyper_prover_dispatcher_fail() {
    let (mut ctx, intent_hash) = setup();
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source_chain = random::<u32>() as u64;
    let invalid_hyper_dispatcher = Pubkey::new_unique();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hash,
        source_chain,
        fulfill_marker,
        dispatcher,
        invalid_hyper_dispatcher,
        hyperlane::MAILBOX_ID,
        random::<[u8; 32]>().into(),
    );

    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidDispatcher)));
}

#[test]
fn prove_intent_invalid_mailbox_not_executable_fail() {
    let (mut ctx, intent_hash) = setup();
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source_chain = random::<u32>() as u64;
    let invalid_mailbox = Pubkey::new_unique();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hash,
        source_chain,
        fulfill_marker,
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        invalid_mailbox,
        random::<[u8; 32]>().into(),
    );

    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintExecutable)));
}

#[test]
fn prove_intent_invalid_mailbox_fail() {
    let (mut ctx, intent_hash) = setup();
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source_chain = random::<u32>() as u64;

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hash,
        source_chain,
        fulfill_marker,
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        portal::ID,
        random::<[u8; 32]>().into(),
    );

    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidMailbox)));
}

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

    let result = ctx.hyper_prover().prove(
        &invalid_dispatcher,
        1,
        [0u8; 32].into(),
        vec![0u8; 32],
        [1u8; 32].into(),
        outbox_pda,
        &unique_message,
        dispatched_message_pda,
    );
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidPortalDispatcher)));
}
