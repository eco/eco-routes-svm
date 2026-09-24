use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};
use local_prover::state::ProofAccount;
use portal::events::{IntentCancelled, IntentFulfilled, IntentProven};
use portal::instructions::PortalError;
use portal::state::{self, FulfillMarker};
use portal::types::{self, Route};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

/// A minimal intent destined for this chain and proven by local-prover, not yet
/// fulfilled.
fn open_intent(ctx: &mut common::Context) -> (Bytes32, Route, Bytes32) {
    let (intent_hash, route, reward) = ctx.rand_minimal_intent(local_prover::ID);

    (intent_hash, route, reward.hash())
}

#[test]
fn cancel_success() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_ok_and(common::contains_event(IntentCancelled::new(intent_hash))));
    let marker = ctx.account::<FulfillMarker>(&fulfill_marker).unwrap();
    assert_eq!(marker.claimant, CANCELLED);
    assert_eq!(marker.payer, ctx.payer.pubkey());
    assert_eq!(marker.deadline, route.deadline);
}

/// `fulfill` still accepts `now == route.deadline`, so `cancel` must not.
#[test]
fn cancel_at_route_deadline_fail() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64);

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::RouteNotExpired)));
    assert!(ctx.get_account(&fulfill_marker).is_none());
}

/// The other half of the boundary: the fulfill and cancel windows meet at
/// `route.deadline` without overlapping.
#[test]
fn fulfill_at_route_deadline_success() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();

    ctx.warp_to_timestamp(route.deadline as i64);

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        claimant,
        state::executor_pda().0,
        fulfill_marker,
        vec![],
        vec![],
    );

    assert!(
        result.is_ok_and(common::contains_event(IntentFulfilled::new(
            intent_hash,
            claimant
        )))
    );
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker)
            .unwrap()
            .claimant,
        claimant
    );
}

#[test]
fn cancel_invalid_portal_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, reward_hash) = open_intent(&mut ctx);
    route.portal = random::<[u8; 32]>().into();
    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::InvalidPortal)));
}

#[test]
fn cancel_invalid_intent_hash_fail() {
    let mut ctx = common::Context::default();
    let (_, route, reward_hash) = open_intent(&mut ctx);
    let wrong_intent_hash: Bytes32 = random::<[u8; 32]>().into();
    let fulfill_marker = FulfillMarker::pda(&wrong_intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);

    let result = ctx
        .portal()
        .cancel_intent(wrong_intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::InvalidIntentHash)));
}

#[test]
fn cancel_invalid_fulfill_marker_fail() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);

    ctx.warp_to_timestamp(route.deadline as i64 + 1);

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, Pubkey::new_unique());

    assert!(result.is_err_and(common::is_error(PortalError::InvalidFulfillMarker)));
}

#[test]
fn cancel_after_fulfill_fail() {
    let mut ctx = common::Context::default();
    let intent = ctx.fulfill_rand_intents(1, local_prover::ID).remove(0);
    let fulfill_marker = FulfillMarker::pda(&intent.intent_hash).0;
    let claimant = ctx
        .account::<FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;

    ctx.warp_to_timestamp(intent.route.deadline as i64 + 1);

    let result = ctx.portal().cancel_intent(
        intent.intent_hash,
        &intent.route,
        intent.reward_hash,
        fulfill_marker,
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentAlreadyFulfilled)));
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker)
            .unwrap()
            .claimant,
        claimant
    );
}

#[test]
fn cancel_twice_fail() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker)
        .unwrap();

    let result = ctx
        .portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::IntentAlreadyFulfilled)));
}

/// Mutual exclusion from the other side: the windows are disjoint, so a
/// cancelled intent is already past the point where `fulfill` would accept it.
#[test]
fn fulfill_after_cancel_fail() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        Pubkey::new_unique().to_bytes().into(),
        state::executor_pda().0,
        fulfill_marker,
        vec![],
        vec![],
    );

    assert!(result.is_err_and(common::is_error(PortalError::RouteExpired)));
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker)
            .unwrap()
            .claimant,
        CANCELLED
    );
}

/// `prove` needs no change: the sentinel rides the existing payload, and the
/// prover records it verbatim.
#[test]
fn prove_cancelled_intent_via_local_prover_success() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward_hash) = open_intent(&mut ctx);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent_hash],
        CHAIN_ID,
        vec![fulfill_marker],
        state::dispatcher_pda(&local_prover::ID).0,
        vec![proof],
    );

    assert!(result.is_ok_and(common::contains_event(IntentProven::new(
        intent_hash,
        CANCELLED
    ))));
    let proof = ctx.account::<ProofAccount>(&proof).unwrap();
    assert!(CANCELLED == proof.0.claimant);
    assert_eq!(proof.0.destination, CHAIN_ID);
}
