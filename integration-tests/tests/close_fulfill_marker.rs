use anchor_lang::Space;
use eco_svm_std::{prover, Bytes32, CHAIN_ID};
use portal::events::FulfillMarkerClosed;
use portal::instructions::PortalError;
use portal::state::FulfillMarker;
use portal::{state, types};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

pub mod common;

/// LiteSVM's default fee structure charges 5000 lamports per signature, and
/// every close transaction here carries exactly one.
const TRANSACTION_FEE: u64 = 5_000;

/// Fulfills `intent_count` minimal intents, returning their hashes and the
/// latest `route.deadline` across them.
fn setup(intent_count: usize) -> (common::Context, Vec<Bytes32>, u64) {
    let mut ctx = common::Context::default();

    let (intent_hashes, deadlines): (Vec<_>, Vec<_>) = (0..intent_count)
        .map(|_| {
            let (_, mut route, mut reward) = ctx.rand_intent();
            route.tokens.clear();
            route.calls.clear();
            route.native_amount = 0;
            reward.prover = local_prover::ID;
            let claimant = Pubkey::new_unique().to_bytes().into();

            let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

            ctx.portal()
                .fulfill_intent(
                    intent_hash,
                    &route,
                    reward.hash(),
                    claimant,
                    state::executor_pda().0,
                    FulfillMarker::pda(&intent_hash).0,
                    vec![],
                    vec![],
                )
                .unwrap();

            (intent_hash, route.deadline)
        })
        .unzip();

    let deadline = deadlines.into_iter().max().unwrap();

    (ctx, intent_hashes, deadline)
}

fn rent_exempt_minimum(ctx: &common::Context) -> u64 {
    ctx.get_sysvar::<Rent>()
        .minimum_balance(8 + FulfillMarker::INIT_SPACE)
}

#[test]
fn close_fulfill_marker_success() {
    let (mut ctx, intent_hashes, deadline) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let payer = ctx.payer.pubkey();
    let rent = rent_exempt_minimum(&ctx);

    assert_eq!(ctx.balance(&fulfill_marker), rent);
    ctx.warp_to_timestamp(deadline as i64 + 1);
    let payer_balance = ctx.balance(&payer);

    let result = ctx
        .portal()
        .close_fulfill_marker(intent_hash, fulfill_marker);

    assert!(
        result.is_ok_and(common::contains_event(FulfillMarkerClosed::new(
            intent_hash,
            payer
        )))
    );
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_none());
    assert_eq!(ctx.balance(&fulfill_marker), 0);
    assert_eq!(ctx.balance(&payer), payer_balance + rent - TRANSACTION_FEE);
}

#[test]
fn close_fulfill_marker_batch_success() {
    let (mut ctx, intent_hashes, deadline) = setup(3);
    let payer = ctx.payer.pubkey();
    let rent = rent_exempt_minimum(&ctx);
    let markers: Vec<_> = intent_hashes
        .iter()
        .map(|intent_hash| (*intent_hash, FulfillMarker::pda(intent_hash).0))
        .collect();

    ctx.warp_to_timestamp(deadline as i64 + 1);
    let payer_balance = ctx.balance(&payer);

    let result = ctx
        .portal()
        .close_fulfill_markers(markers.clone(), payer, vec![]);

    assert!(result.is_ok());
    markers.iter().for_each(|(_, fulfill_marker)| {
        assert!(ctx.account::<FulfillMarker>(fulfill_marker).is_none());
    });
    assert_eq!(
        ctx.balance(&payer),
        payer_balance + rent * markers.len() as u64 - TRANSACTION_FEE
    );
}

#[test]
fn close_fulfill_marker_before_deadline_fail() {
    let (mut ctx, intent_hashes, deadline) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    // `fulfill` requires `route.deadline >= now`, so the marker is still the
    // only thing preventing a second fulfill up to and including the deadline.
    ctx.warp_to_timestamp(deadline as i64);

    let result = ctx
        .portal()
        .close_fulfill_marker(intent_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::RouteNotExpired)));
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_some());
}

#[test]
fn close_fulfill_marker_wrong_payer_fail() {
    let (mut ctx, intent_hashes, deadline) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let stranger = Keypair::new();

    ctx.warp_to_timestamp(deadline as i64 + 1);

    let result = ctx.portal().close_fulfill_markers(
        vec![(intent_hash, fulfill_marker)],
        stranger.pubkey(),
        vec![&stranger],
    );

    assert!(result.is_err_and(common::is_error(PortalError::InvalidFulfillMarkerPayer)));
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_some());
}

#[test]
fn close_fulfill_marker_wrong_intent_hash_fail() {
    let (mut ctx, intent_hashes, deadline) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let wrong_intent_hash = rand::random::<[u8; 32]>().into();

    ctx.warp_to_timestamp(deadline as i64 + 1);

    let result = ctx
        .portal()
        .close_fulfill_marker(wrong_intent_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(PortalError::InvalidFulfillMarker)));
    assert!(ctx.account::<FulfillMarker>(&fulfill_marker).is_some());
}

#[test]
fn close_fulfill_marker_twice_fail() {
    let (mut ctx, intent_hashes, deadline) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(deadline as i64 + 1);
    ctx.portal()
        .close_fulfill_marker(intent_hash, fulfill_marker)
        .unwrap();

    let result = ctx
        .portal()
        .close_fulfill_marker(intent_hash, fulfill_marker);

    assert!(result.is_err_and(common::is_error(
        anchor_lang::error::ErrorCode::AccountNotInitialized
    )));
}

/// The loss mode the close instruction is deliberately not protected against:
/// `prove` reads the claimant out of the marker and has no other source for it,
/// so an intent closed before it is proven can never be proven and its reward
/// is never claimable. Only the payer can close, so the timing is the closer's
/// own liability — pinned here so the consequence stays visible.
#[test]
fn prove_after_close_fail() {
    let (mut ctx, intent_hashes, deadline) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(deadline as i64 + 1);
    ctx.portal()
        .close_fulfill_marker(intent_hash, fulfill_marker)
        .unwrap();

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent_hash],
        CHAIN_ID,
        vec![fulfill_marker],
        state::dispatcher_pda().0,
        vec![prover::Proof::pda(&intent_hash, &local_prover::ID).0],
    );

    assert!(result.is_err_and(common::is_error(PortalError::InvalidFulfillMarker)));
}
