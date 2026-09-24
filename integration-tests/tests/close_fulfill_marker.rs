use anchor_lang::Space;
use eco_svm_std::{prover, CANCELLED, CHAIN_ID};
use local_prover::state::ProofAccount;
use portal::events::FulfillMarkerClosed;
use portal::instructions::PortalError;
use portal::state::{self, FulfillMarker, FulfillTombstone};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

pub mod common;

/// Every transaction that asserts a balance delta below carries exactly one
/// signature, and LiteSVM's default fee structure charges 5000 lamports each.
const TRANSACTION_FEE: u64 = 5_000;

fn marker_rent(ctx: &common::Context) -> u64 {
    ctx.get_sysvar::<Rent>()
        .minimum_balance(8 + FulfillMarker::INIT_SPACE)
}

fn tombstone_rent(ctx: &common::Context) -> u64 {
    ctx.get_sysvar::<Rent>()
        .minimum_balance(8 + FulfillTombstone::INIT_SPACE)
}

#[test]
fn close_fulfill_marker_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let payer = ctx.payer.pubkey();
    let refund = marker_rent(&ctx) - tombstone_rent(&ctx);
    let claimant = ctx
        .account::<FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;

    assert_eq!(ctx.balance(&fulfill_marker), marker_rent(&ctx));
    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);
    let payer_balance = ctx.balance(&payer);

    let result = ctx
        .portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker);

    assert!(
        result.is_ok_and(common::contains_event(FulfillMarkerClosed::new(
            intent.intent_hash,
            payer,
            claimant,
            refund,
        )))
    );
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_none());
    assert_eq!(
        ctx.account::<FulfillTombstone>(&fulfill_marker).unwrap(),
        FulfillTombstone::new(claimant)
    );
    assert_eq!(
        ctx.get_account(&fulfill_marker).unwrap().data.len(),
        8 + FulfillTombstone::INIT_SPACE
    );
    assert_eq!(ctx.balance(&fulfill_marker), tombstone_rent(&ctx));
    assert_eq!(
        ctx.balance(&payer),
        payer_balance + refund - TRANSACTION_FEE
    );
}

#[test]
fn close_fulfill_marker_batch_success() {
    let mut ctx = common::Context::default();
    let intents = ctx.fulfill_rand_intents(3, local_prover::ID);
    let payer = ctx.payer.pubkey();
    let refund = marker_rent(&ctx) - tombstone_rent(&ctx);
    let deadline = intents
        .iter()
        .map(|intent| intent.route.deadline)
        .max()
        .unwrap();
    let markers: Vec<_> = intents
        .iter()
        .map(|intent| {
            (
                intent.intent_hash,
                FulfillMarker::pda(&intent.intent_hash).0,
            )
        })
        .collect();

    ctx.warp_to_timestamp(deadline as i64 + 1);
    let payer_balance = ctx.balance(&payer);

    let result = ctx
        .portal()
        .close_fulfill_markers(markers.clone(), payer, vec![]);

    assert!(result.is_ok());
    markers.iter().for_each(|(_, fulfill_marker)| {
        assert!(ctx.account::<FulfillTombstone>(fulfill_marker).is_some());
    });
    assert_eq!(
        ctx.balance(&payer),
        payer_balance + refund * markers.len() as u64 - TRANSACTION_FEE
    );
}

#[test]
fn close_fulfill_marker_before_deadline_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;

    // `fulfill` requires `route.deadline >= now`, so up to and including the
    // deadline the marker is still the only double-fulfill guard.
    ctx.warp_to_timestamp(intent.route.deadline as i64);

    let result = ctx
        .portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::RouteNotExpired)));
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_some());
}

/// The deadline gate's entire justification: a closed marker does not reopen
/// the intent to a second fulfill, because past `route.deadline` `fulfill`
/// itself rejects. The two checks live in different instructions and are
/// coupled only through `marker.deadline == route.deadline`, so pin it.
#[test]
fn fulfill_after_close_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);
    ctx.portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().fulfill_intent(
        intent.intent_hash,
        &intent.route,
        intent.reward_hash,
        Pubkey::new_unique().to_bytes().into(),
        state::executor_pda().0,
        fulfill_marker,
        vec![],
        vec![],
    );

    assert!(result.is_err_and(common::is_error(PortalError::RouteExpired)));
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_none());
    assert!(ctx.account::<FulfillTombstone>(&fulfill_marker).is_some());
}

#[test]
fn close_fulfill_marker_wrong_payer_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let stranger = Keypair::new();

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);

    let result = ctx.portal().close_fulfill_markers(
        vec![(intent.intent_hash, fulfill_marker)],
        stranger.pubkey(),
        vec![&stranger],
    );

    assert!(result.is_err_and(common::is_error(PortalError::InvalidFulfillMarkerPayer)));
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_some());
}

#[test]
fn close_fulfill_marker_wrong_intent_hash_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let wrong_intent_hash = rand::random::<[u8; 32]>().into();

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);

    let result = ctx
        .portal()
        .close_fulfill_marker(wrong_intent_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::InvalidFulfillMarker)));
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_some());
}

#[test]
fn close_fulfill_marker_twice_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);
    ctx.portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker)
        .unwrap();

    let result = ctx
        .portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::InvalidFulfillMarker)));
}

/// The tombstone keeps the claimant, so closing no longer strands an unproven
/// fulfillment: the solver can still prove after reclaiming rent.
#[test]
fn prove_after_close_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let proof = prover::Proof::pda(&intent.intent_hash, &local_prover::ID).0;
    let claimant = ctx
        .account::<FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);
    ctx.portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent.intent_hash],
        CHAIN_ID,
        vec![fulfill_marker],
        state::dispatcher_pda(&local_prover::ID).0,
        vec![proof],
    );

    assert!(result.is_ok());
    assert!(claimant == ctx.account::<ProofAccount>(&proof).unwrap().0.claimant);
}

/// A closed marker must not read as "never fulfilled": `cancel` still finds
/// the PDA occupied, and the failed attempt leaves the solver's claimant intact.
#[test]
fn cancel_after_close_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let claimant = ctx
        .account::<FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);
    ctx.portal()
        .close_fulfill_marker(intent.intent_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().cancel_intent(
        intent.intent_hash,
        &intent.route,
        intent.reward_hash,
        fulfill_marker,
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentAlreadyFulfilled)));
    assert_eq!(
        ctx.account::<FulfillTombstone>(&fulfill_marker).unwrap(),
        FulfillTombstone::new(claimant)
    );
}

/// A cancelled marker closes like any other and still proves as a cancellation.
#[test]
fn close_cancelled_marker_keeps_sentinel_success() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward) = ctx.rand_minimal_intent(local_prover::ID);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward.hash(), fulfill_marker)
        .unwrap();

    let result = ctx
        .portal()
        .close_fulfill_marker(intent_hash, fulfill_marker);

    assert!(result.is_ok());
    assert_eq!(
        ctx.account::<FulfillTombstone>(&fulfill_marker).unwrap(),
        FulfillTombstone::new(CANCELLED)
    );
}
