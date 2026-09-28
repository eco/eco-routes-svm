use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::solana_program::system_instruction;
use anchor_lang::system_program;
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};
use local_prover::state::ProofAccount;
use portal::events::{IntentCancelled, IntentFulfilled, IntentProven};
use portal::instructions::PortalError;
use portal::state::{self, FulfillMarker};
use portal::types::{self, Call, Calldata, CalldataWithAccounts, Route};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

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

/// Solana's wire limit for a serialized legacy or v0 transaction.
const PACKET_DATA_SIZE: usize = 1232;

const TRANSFER_CALLS: u64 = 8;
const TRANSFER_LAMPORTS: u64 = 10_000_000;

/// A route of `TRANSFER_CALLS` native transfers that all reuse the executor,
/// the recipient and the system program as call accounts. Returns the source
/// (canonical) route, the destination (compact) route and one call's account
/// metas, which every call repeats.
fn repeated_transfer_routes(ctx: &mut common::Context) -> (Route, Route, Vec<AccountMeta>) {
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = TRANSFER_CALLS * TRANSFER_LAMPORTS;
    let executor = state::executor_pda().0;
    let recipient = Pubkey::new_unique();

    let calldata = Calldata {
        data: system_instruction::transfer(&executor, &recipient, TRANSFER_LAMPORTS).data,
        account_count: 3,
    };
    let call_accounts = vec![
        AccountMeta::new(executor, false),
        AccountMeta::new(recipient, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    let canonical = CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let mut source_route = route.clone();
    source_route.calls = (0..TRANSFER_CALLS)
        .map(|_| Call {
            target: system_program::ID.to_bytes().into(),
            data: borsh::to_vec(&canonical).unwrap(),
        })
        .collect();
    let mut destination_route = route;
    destination_route.calls = (0..TRANSFER_CALLS)
        .map(|_| Call {
            target: system_program::ID.to_bytes().into(),
            data: borsh::to_vec(&calldata).unwrap(),
        })
        .collect();

    (source_route, destination_route, call_accounts)
}

fn repeated(call_accounts: &[AccountMeta]) -> Vec<AccountMeta> {
    (0..TRANSFER_CALLS)
        .flat_map(|_| call_accounts.to_vec())
        .collect()
}

fn serialized_len(transaction: &Transaction) -> usize {
    bincode::serialize(transaction).unwrap().len()
}

/// A route whose calls reuse account metas fits a `fulfill` transaction; the
/// equivalent expired route must fit a `cancel` one too. Supplying the
/// canonical route inline, as `cancel` used to, would not.
#[test]
fn cancel_route_that_fits_fulfill_fits_transaction_success() {
    let mut ctx = common::Context::default();
    let reward_hash: Bytes32 = random::<[u8; 32]>().into();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    // Fulfill one instance before its deadline
    let (source_route, destination_route, call_accounts) = repeated_transfer_routes(&mut ctx);
    let fulfilled_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfilled_marker = FulfillMarker::pda(&fulfilled_hash).0;
    let solver = ctx.solver.pubkey();
    ctx.airdrop(&solver, destination_route.native_amount)
        .unwrap();
    let fulfill = ctx.portal().fulfill_intent_transaction(
        fulfilled_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfilled_marker,
        vec![],
        repeated(&call_accounts),
        vec![],
    );
    assert!(serialized_len(&fulfill) <= PACKET_DATA_SIZE);
    assert!(ctx
        .send_transaction(fulfill)
        .is_ok_and(common::contains_event(IntentFulfilled::new(
            fulfilled_hash,
            claimant
        ))));

    // Cancel and prove an equivalent instance after its deadline
    let (source_route, destination_route, call_accounts) = repeated_transfer_routes(&mut ctx);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
    ctx.warp_to_timestamp(destination_route.deadline as i64 + 1);

    let cancel = ctx.portal().cancel_intent_transaction(
        intent_hash,
        &destination_route,
        reward_hash,
        fulfill_marker,
        repeated(&call_accounts),
    );
    let cancel_len = serialized_len(&cancel);
    let inline_canonical_len = cancel_len + borsh::to_vec(&source_route).unwrap().len()
        - borsh::to_vec(&destination_route).unwrap().len();
    assert!(cancel_len <= PACKET_DATA_SIZE);
    assert!(inline_canonical_len > PACKET_DATA_SIZE);
    assert!(ctx
        .send_transaction(cancel)
        .is_ok_and(common::contains_event(IntentCancelled::new(intent_hash))));
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker)
            .unwrap()
            .claimant,
        CANCELLED
    );

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
    assert!(CANCELLED == ctx.account::<ProofAccount>(&proof).unwrap().0.claimant);
}

/// The flags are part of the committed route, so misreporting one changes the
/// rebuilt hash
#[test]
fn cancel_wrong_account_flags_fail() {
    let mut ctx = common::Context::default();
    let reward_hash: Bytes32 = random::<[u8; 32]>().into();
    let (source_route, destination_route, mut call_accounts) = repeated_transfer_routes(&mut ctx);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    ctx.warp_to_timestamp(destination_route.deadline as i64 + 1);

    call_accounts[1].is_writable = false;
    let result = ctx.portal().cancel_intent_with_call_accounts(
        intent_hash,
        &destination_route,
        reward_hash,
        fulfill_marker,
        repeated(&call_accounts),
    );

    assert!(result.is_err_and(common::is_error(PortalError::InvalidIntentHash)));
    assert!(ctx.get_account(&fulfill_marker).is_none());
}

#[test]
fn cancel_extra_call_account_fail() {
    let mut ctx = common::Context::default();
    let reward_hash: Bytes32 = random::<[u8; 32]>().into();
    let (source_route, destination_route, call_accounts) = repeated_transfer_routes(&mut ctx);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    ctx.warp_to_timestamp(destination_route.deadline as i64 + 1);

    let mut accounts = repeated(&call_accounts);
    accounts.push(AccountMeta::new_readonly(Pubkey::new_unique(), false));
    let result = ctx.portal().cancel_intent_with_call_accounts(
        intent_hash,
        &destination_route,
        reward_hash,
        fulfill_marker,
        accounts,
    );

    assert!(result.is_err_and(common::is_error(PortalError::InvalidCalldata)));
}

#[test]
fn cancel_missing_call_account_fail() {
    let mut ctx = common::Context::default();
    let reward_hash: Bytes32 = random::<[u8; 32]>().into();
    let (source_route, destination_route, call_accounts) = repeated_transfer_routes(&mut ctx);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    ctx.warp_to_timestamp(destination_route.deadline as i64 + 1);

    let mut accounts = repeated(&call_accounts);
    accounts.pop();
    let result = ctx.portal().cancel_intent_with_call_accounts(
        intent_hash,
        &destination_route,
        reward_hash,
        fulfill_marker,
        accounts,
    );

    assert!(result.is_err_and(common::is_error(PortalError::InvalidCalldata)));
}
