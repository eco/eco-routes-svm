use eco_svm_std::{prover, CHAIN_ID};
use local_prover::instructions::LocalProverError;
use local_prover::state::ProofAccount;
use portal::state;
use portal::types::{self, Intent};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::system_instruction::SystemError;

pub mod common;

fn setup() -> (common::Context, Intent) {
    let mut ctx = common::Context::default();
    let mut intent = ctx.rand_intent();
    intent.route.tokens.clear();
    intent.route.calls.clear();
    intent.reward.prover = local_prover::ID;
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    let intent_hash = types::intent_hash(CHAIN_ID, &intent.route.hash(), &intent.reward.hash());
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    ctx.portal()
        .fulfill_intent(
            &intent.route,
            intent.reward.hash(),
            claimant,
            executor,
            fulfill_marker,
            vec![],
            vec![],
        )
        .unwrap();

    (ctx, intent)
}

#[test]
fn prove_intent_success() {
    let (mut ctx, intent) = setup();
    let intent_hash = types::intent_hash(CHAIN_ID, &intent.route.hash(), &intent.reward.hash());
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source_chain = CHAIN_ID;
    let claimant = ctx
        .account::<state::FulfillMarker>(&fulfill_marker)
        .unwrap()
        .claimant;
    let proof = prover::Proof::pda(&intent_hash, &local_prover::ID).0;

    let result = ctx.portal().prove_intent_via_local_prover(
        intent_hash,
        source_chain,
        fulfill_marker,
        dispatcher,
        proof,
    );
    assert!(
        result.is_ok_and(common::contains_cpi_event(prover::IntentFulfilled::new(
            intent_hash,
            claimant
        )))
    );

    let proof_pda = prover::Proof::pda(&intent_hash, &local_prover::ID).0;
    let proof: ProofAccount = ctx.account(&proof_pda).unwrap();
    assert_eq!(CHAIN_ID, proof.0.destination_chain);
    assert_eq!(claimant, proof.0.claimant);
}

#[test]
fn prove_intent_invalid_source_chain_fail() {
    let (mut ctx, intent) = setup();
    let intent_hash = types::intent_hash(CHAIN_ID, &intent.route.hash(), &intent.reward.hash());
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let invalid_source_chain = CHAIN_ID + 1;
    let proof = prover::Proof::pda(&intent_hash, &local_prover::ID).0;

    let result = ctx.portal().prove_intent_via_local_prover(
        intent_hash,
        invalid_source_chain,
        fulfill_marker,
        dispatcher,
        proof,
    );
    assert!(result.is_err_and(common::is_error(LocalProverError::InvalidSourceChain)));
}

#[test]
fn prove_invalid_portal_dispatcher_fail() {
    let mut ctx = common::Context::default();
    let invalid_dispatcher = ctx.payer.insecure_clone();
    let intent_hash = [1u8; 32].into();
    let source_chain = CHAIN_ID;
    let claimant = [2u8; 32].into();

    let result = ctx.local_prover().prove(
        &invalid_dispatcher,
        source_chain,
        intent_hash,
        vec![],
        claimant,
    );
    assert!(result.is_err_and(common::is_error(LocalProverError::InvalidPortalDispatcher)));
}

#[test]
fn prove_intent_already_proven_fail() {
    let (mut ctx, intent) = setup();
    let intent_hash = types::intent_hash(CHAIN_ID, &intent.route.hash(), &intent.reward.hash());
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source_chain = CHAIN_ID;
    let proof = prover::Proof::pda(&intent_hash, &local_prover::ID).0;

    ctx.portal()
        .prove_intent_via_local_prover(intent_hash, source_chain, fulfill_marker, dispatcher, proof)
        .unwrap();

    let result = ctx.portal().prove_intent_via_local_prover(
        intent_hash,
        source_chain,
        fulfill_marker,
        dispatcher,
        proof,
    );
    assert!(result.is_err_and(common::is_error(SystemError::AccountAlreadyInUse as u32)));
}
