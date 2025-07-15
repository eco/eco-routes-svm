use eco_svm_std::{prover, CHAIN_ID};
use local_prover::instructions::LocalProverError;
use local_prover::state::ProofAccount;
use portal::{state, types};
use solana_sdk::pubkey::Pubkey;

pub mod common;

fn setup(intent_count: usize) -> (common::Context, Vec<eco_svm_std::Bytes32>) {
    let mut ctx = common::Context::default();

    let intent_hashes = (0..intent_count)
        .map(|_| {
            let (_, mut route, mut reward) = ctx.rand_intent();
            route.tokens.clear();
            route.calls.clear();
            reward.prover = local_prover::ID;
            let claimant = Pubkey::new_unique().to_bytes().into();
            let executor = state::executor_pda().0;

            let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
            let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

            ctx.portal()
                .fulfill_intent(
                    intent_hash,
                    &route,
                    reward.hash(),
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
    let source = CHAIN_ID;
    let claimants = fulfill_markers
        .iter()
        .map(|marker| {
            ctx.account::<state::FulfillMarker>(marker)
                .unwrap()
                .claimant
        })
        .collect::<Vec<_>>();
    let proofs = intent_hashes
        .iter()
        .map(|hash| prover::Proof::pda(hash, &local_prover::ID).0)
        .collect::<Vec<_>>();

    let result = ctx.portal().prove_intent_via_local_prover(
        intent_hashes.clone(),
        source,
        fulfill_markers,
        dispatcher,
        proofs,
    );
    intent_hashes
        .into_iter()
        .zip(claimants)
        .for_each(|(intent_hash, claimant)| {
            assert!(result.clone().is_ok_and(common::contains_cpi_event(
                prover::IntentProven::new(intent_hash, CHAIN_ID, CHAIN_ID)
            )));

            let proof_pda = prover::Proof::pda(&intent_hash, &local_prover::ID).0;
            let proof: ProofAccount = ctx.account(&proof_pda).unwrap();
            assert_eq!(CHAIN_ID, proof.0.destination);
            assert_eq!(
                Pubkey::try_from(claimant.as_ref()).unwrap(),
                proof.0.claimant
            );
        });
}

#[test]
fn prove_intent_multiple_success() {
    let (mut ctx, intent_hashes) = setup(3);
    let fulfill_markers = intent_hashes
        .iter()
        .map(|hash| state::FulfillMarker::pda(hash).0)
        .collect::<Vec<_>>();
    let dispatcher = state::dispatcher_pda().0;
    let source = CHAIN_ID;
    let claimants = fulfill_markers
        .iter()
        .map(|marker| {
            ctx.account::<state::FulfillMarker>(marker)
                .unwrap()
                .claimant
        })
        .collect::<Vec<_>>();
    let proofs = intent_hashes
        .iter()
        .map(|hash| prover::Proof::pda(hash, &local_prover::ID).0)
        .collect::<Vec<_>>();

    let result = ctx.portal().prove_intent_via_local_prover(
        intent_hashes.clone(),
        source,
        fulfill_markers,
        dispatcher,
        proofs,
    );
    intent_hashes
        .into_iter()
        .zip(claimants)
        .for_each(|(intent_hash, claimant)| {
            assert!(result.clone().is_ok_and(common::contains_cpi_event(
                prover::IntentProven::new(intent_hash, CHAIN_ID, CHAIN_ID)
            )));

            let proof_pda = prover::Proof::pda(&intent_hash, &local_prover::ID).0;
            let proof: ProofAccount = ctx.account(&proof_pda).unwrap();
            assert_eq!(CHAIN_ID, proof.0.destination);
            assert_eq!(
                Pubkey::try_from(claimant.as_ref()).unwrap(),
                proof.0.claimant
            );
        });
}

#[test]
fn prove_intent_invalid_source_fail() {
    let (mut ctx, intent_hashes) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let invalid_source = CHAIN_ID + 1;
    let proof = prover::Proof::pda(&intent_hash, &local_prover::ID).0;

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent_hash],
        invalid_source,
        vec![fulfill_marker],
        dispatcher,
        vec![proof],
    );
    assert!(result.is_err_and(common::is_error(LocalProverError::InvalidSource)));
}

#[test]
fn prove_invalid_portal_dispatcher_fail() {
    let mut ctx = common::Context::default();
    let invalid_dispatcher = ctx.payer.insecure_clone();
    let intent_hash = [1u8; 32].into();
    let source = CHAIN_ID;
    let claimant = [2u8; 32].into();

    let result = ctx.local_prover().prove(
        &invalid_dispatcher,
        source,
        vec![(intent_hash, claimant)].into(),
        vec![],
        vec![],
    );
    assert!(result.is_err_and(common::is_error(LocalProverError::InvalidPortalDispatcher)));
}

#[test]
fn prove_intent_already_proven_fail() {
    let (mut ctx, intent_hashes) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source = CHAIN_ID;
    let proof = prover::Proof::pda(&intent_hash, &local_prover::ID).0;

    ctx.portal()
        .prove_intent_via_local_prover(
            vec![intent_hash],
            source,
            vec![fulfill_marker],
            dispatcher,
            vec![proof],
        )
        .unwrap();

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent_hash],
        source,
        vec![fulfill_marker],
        dispatcher,
        vec![proof],
    );
    assert!(result.is_err_and(common::is_error(LocalProverError::IntentAlreadyProven)));
}

#[test]
fn prove_intent_empty_intent_hashes_fail() {
    let mut ctx = common::Context::default();
    let dispatcher = state::dispatcher_pda().0;
    let source = CHAIN_ID;

    let result =
        ctx.portal()
            .prove_intent_via_local_prover(vec![], source, vec![], dispatcher, vec![]);

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::EmptyIntentHashes
    )));
}

#[test]
fn prove_intent_insufficient_proofs_fail() {
    let (mut ctx, intent_hashes) = setup(2);
    let intent_hash_1 = intent_hashes[0];
    let intent_hash_2 = intent_hashes[1];
    let fulfill_marker_1 = state::FulfillMarker::pda(&intent_hash_1).0;
    let fulfill_marker_2 = state::FulfillMarker::pda(&intent_hash_2).0;
    let dispatcher = state::dispatcher_pda().0;
    let source = CHAIN_ID;
    let proof_1 = prover::Proof::pda(&intent_hash_1, &local_prover::ID).0;

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent_hash_1, intent_hash_2],
        source,
        vec![fulfill_marker_1, fulfill_marker_2],
        dispatcher,
        vec![proof_1],
    );

    assert!(result.is_err_and(common::is_error(LocalProverError::InvalidProof)));
}

#[test]
fn prove_intent_wrong_proof_pda_fail() {
    let (mut ctx, intent_hashes) = setup(1);
    let intent_hash = intent_hashes[0];
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;
    let dispatcher = state::dispatcher_pda().0;
    let source = CHAIN_ID;
    let wrong_intent_hash: eco_svm_std::Bytes32 = rand::random::<[u8; 32]>().into();
    let wrong_proof = prover::Proof::pda(&wrong_intent_hash, &local_prover::ID).0;

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent_hash],
        source,
        vec![fulfill_marker],
        dispatcher,
        vec![wrong_proof],
    );

    assert!(result.is_err_and(common::is_error(LocalProverError::InvalidProof)));
}
