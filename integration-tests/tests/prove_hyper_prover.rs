use std::vec;

use anchor_lang::error::ErrorCode;
use anchor_lang::AnchorDeserialize;
use eco_svm_std::prover::{IntentHashClaimant, ProofData};
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

fn setup(intent_count: usize) -> (common::Context, Vec<Bytes32>) {
    let mut ctx = common::Context::default();

    let intent_hashes = (0..intent_count)
        .map(|_| {
            let (_, mut route, _) = ctx.rand_intent();
            route.tokens.clear();
            route.calls.clear();
            let reward_hash = rand::random::<[u8; 32]>().into();
            let claimant = Pubkey::new_unique().to_bytes().into();
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

            intent_hash
        })
        .collect();

    (ctx, intent_hashes)
}

#[test]
fn prove_intent_success() {
    let (mut ctx, intent_hashes) = setup(1);
    let fulfill_markers = intent_hashes
        .iter()
        .map(|hash| state::FulfillMarker::pda(hash).0)
        .collect::<Vec<_>>();
    let dispatcher = state::dispatcher_pda().0;
    let source = random::<u32>() as u64;
    let source_prover = random::<[u8; 32]>();
    let claimants = fulfill_markers
        .iter()
        .map(|marker| {
            ctx.account::<state::FulfillMarker>(marker)
                .unwrap()
                .claimant
        })
        .collect::<Vec<_>>();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hashes.clone(),
        source,
        fulfill_markers,
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        source_prover.into(),
    );
    intent_hashes
        .iter()
        .zip(claimants.clone())
        .for_each(|(intent_hash, claimant)| {
            assert!(result.clone().is_ok_and(common::contains_event_and_msg(
                IntentProven::new(*intent_hash, claimant),
                "Dispatched message"
            )));
        });

    let tx = result.unwrap();
    let mailbox_dispatch: Vec<_> = tx
        .inner_instructions
        .into_iter()
        .flat_map(IntoIterator::into_iter)
        .filter_map(|ix| MailboxInstruction::try_from_slice(ix.instruction.data.as_slice()).ok())
        .collect();
    assert_eq!(mailbox_dispatch.len(), 1);
    let proof_data = ProofData::new(
        CHAIN_ID,
        intent_hashes
            .into_iter()
            .zip(claimants)
            .map(|(intent_hash, claimant)| {
                eco_svm_std::prover::IntentHashClaimant::new(intent_hash, claimant)
            })
            .collect(),
    );

    match mailbox_dispatch.first().unwrap() {
        MailboxInstruction::OutboxDispatch(msg) => {
            assert_eq!(msg.sender, hyper_prover::state::dispatcher_pda().0);
            assert_eq!(msg.destination_domain, source as u32);
            assert_eq!(msg.recipient, source_prover);
            assert_eq!(msg.message_body, proof_data.to_bytes());
        }
        _ => panic!("expected OutboxDispatch instruction"),
    }
}

#[test]
fn prove_intent_multiple_success() {
    let (mut ctx, intent_hashes) = setup(3);
    let fulfill_markers = intent_hashes
        .iter()
        .map(|hash| state::FulfillMarker::pda(hash).0)
        .collect::<Vec<_>>();
    let dispatcher = state::dispatcher_pda().0;
    let source = random::<u32>() as u64;
    let source_prover = random::<[u8; 32]>();
    let claimants = fulfill_markers
        .iter()
        .map(|marker| {
            ctx.account::<state::FulfillMarker>(marker)
                .unwrap()
                .claimant
        })
        .collect::<Vec<_>>();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hashes.clone(),
        source,
        fulfill_markers,
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        source_prover.into(),
    );
    intent_hashes
        .iter()
        .zip(claimants.clone())
        .for_each(|(intent_hash, claimant)| {
            assert!(result.clone().is_ok_and(common::contains_event_and_msg(
                IntentProven::new(*intent_hash, claimant),
                "Dispatched message"
            )));
        });

    let tx = result.unwrap();
    let mailbox_dispatch: Vec<_> = tx
        .inner_instructions
        .into_iter()
        .flat_map(IntoIterator::into_iter)
        .filter_map(|ix| MailboxInstruction::try_from_slice(ix.instruction.data.as_slice()).ok())
        .collect();
    assert_eq!(mailbox_dispatch.len(), 1);
    let proof_data = ProofData::new(
        CHAIN_ID,
        intent_hashes
            .into_iter()
            .zip(claimants)
            .map(|(intent_hash, claimant)| IntentHashClaimant::new(intent_hash, claimant))
            .collect(),
    );

    match mailbox_dispatch.first().unwrap() {
        MailboxInstruction::OutboxDispatch(msg) => {
            assert_eq!(msg.sender, hyper_prover::state::dispatcher_pda().0);
            assert_eq!(msg.destination_domain, source as u32);
            assert_eq!(msg.recipient, source_prover);
            assert_eq!(msg.message_body, proof_data.to_bytes());
        }
        _ => panic!("expected OutboxDispatch instruction"),
    }
}

#[test]
fn prove_intent_invalid_dispatcher_fail() {
    let (mut ctx, intent_hashes) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let invalid_dispatcher = Pubkey::new_unique();
    let source = random::<u32>() as u64;

    let result = ctx.portal().prove_intent_via_hyper_prover(
        vec![intent_hash],
        source,
        vec![fulfill_marker],
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
    let source = random::<u32>() as u64;

    let result = ctx.portal().prove_intent_via_hyper_prover(
        vec![intent_hash],
        source,
        vec![fulfill_marker],
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        random::<[u8; 32]>().into(),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidFulfillMarker
    )));
}

#[test]
fn prove_intent_insufficient_fulfill_markers_fail() {
    let (mut ctx, intent_hashes) = setup(2);
    let fulfill_marker_1 = state::FulfillMarker::pda(&intent_hashes[0]).0;
    let dispatcher = state::dispatcher_pda().0;
    let source = random::<u32>() as u64;
    let source_prover = random::<[u8; 32]>();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        intent_hashes,
        source,
        vec![fulfill_marker_1],
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        source_prover.into(),
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidFulfillMarker
    )));
}

#[test]
fn prove_intent_wrong_fulfill_marker_pda_fail() {
    let (mut ctx, intent_hashes) = setup(1);
    let intent_hash = intent_hashes[0];
    let wrong_fulfill_marker = state::FulfillMarker::pda(&random::<[u8; 32]>().into()).0;
    let dispatcher = state::dispatcher_pda().0;
    let source = random::<u32>() as u64;
    let source_prover = random::<[u8; 32]>();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        vec![intent_hash],
        source,
        vec![wrong_fulfill_marker],
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        source_prover.into(),
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidFulfillMarker
    )));
}

#[test]
fn prove_intent_invalid_hyper_prover_dispatcher_fail() {
    let (mut ctx, intent_hashes) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source = random::<u32>() as u64;
    let invalid_hyper_dispatcher = Pubkey::new_unique();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        vec![intent_hash],
        source,
        vec![fulfill_marker],
        dispatcher,
        invalid_hyper_dispatcher,
        hyperlane::MAILBOX_ID,
        random::<[u8; 32]>().into(),
    );

    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidDispatcher)));
}

#[test]
fn prove_intent_invalid_mailbox_not_executable_fail() {
    let (mut ctx, intent_hashes) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source = random::<u32>() as u64;
    let invalid_mailbox = Pubkey::new_unique();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        vec![intent_hash],
        source,
        vec![fulfill_marker],
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        invalid_mailbox,
        random::<[u8; 32]>().into(),
    );

    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintExecutable)));
}

#[test]
fn prove_intent_invalid_mailbox_fail() {
    let (mut ctx, intent_hashes) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source = random::<u32>() as u64;

    let result = ctx.portal().prove_intent_via_hyper_prover(
        vec![intent_hash],
        source,
        vec![fulfill_marker],
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
        ProofData::new(
            1,
            vec![IntentHashClaimant::new([0u8; 32].into(), [1u8; 32].into())],
        ),
        vec![0u8; 32],
        outbox_pda,
        &unique_message,
        dispatched_message_pda,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidPortalDispatcher)));
}

#[test]
fn prove_intent_empty_intent_hashes_fail() {
    let mut ctx = common::Context::default();
    let dispatcher = state::dispatcher_pda().0;
    let source = random::<u32>() as u64;
    let source_prover = random::<[u8; 32]>();

    let result = ctx.portal().prove_intent_via_hyper_prover(
        vec![],
        source,
        vec![],
        dispatcher,
        hyper_prover::state::dispatcher_pda().0,
        hyperlane::MAILBOX_ID,
        source_prover.into(),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::EmptyIntentHashes
    )));
}
