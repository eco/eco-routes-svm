use eco_svm_std::prover::{IntentHashClaimant, ProofData};
use eco_svm_std::CHAIN_ID;
use polymer_prover::instructions::{
    prove_log_line, prove_log_payload, PolymerProverError, MAX_INTENTS_PER_PROVE,
};
use portal::state;
use solana_sdk::signature::Keypair;

pub mod common;

const EVM_SOURCE_CHAIN: u64 = 8453;
/// Portal's `prove` loads one `FulfillMarker` per intent and the prover hex-encodes
/// each; a full batch needs well past the default 200k-per-instruction budget.
const MAX_INTENTS_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

#[test]
fn prove_via_portal_emits_one_polymer_log_per_intent() {
    let mut ctx = common::Context::default();
    let intents = ctx.fulfill_rand_intents(3, polymer_prover::ID);
    let intent_hashes: Vec<_> = intents.iter().map(|intent| intent.intent_hash).collect();
    let fulfill_markers: Vec<_> = intent_hashes
        .iter()
        .map(|hash| state::FulfillMarker::pda(hash).0)
        .collect();
    let claimants: Vec<_> = fulfill_markers
        .iter()
        .map(|marker| {
            ctx.account::<state::FulfillMarker>(marker)
                .unwrap()
                .claimant
        })
        .collect();

    let result = ctx
        .portal()
        .prove_intent_via_program(
            polymer_prover::ID,
            intent_hashes.clone(),
            EVM_SOURCE_CHAIN,
            fulfill_markers,
            state::dispatcher_pda(&polymer_prover::ID).0,
            vec![],
            vec![],
        )
        .unwrap();

    for (intent_hash, claimant) in intent_hashes.into_iter().zip(claimants) {
        let payload = prove_log_payload(
            EVM_SOURCE_CHAIN,
            CHAIN_ID,
            &IntentHashClaimant::new(intent_hash, claimant),
        );
        let expected = format!(
            "Program log: {}",
            prove_log_line(&polymer_prover::ID, &payload)
        );
        assert!(
            result.logs.contains(&expected),
            "missing log line {expected}\nlogs: {:#?}",
            result.logs
        );
    }
}

#[test]
fn prove_invalid_portal_dispatcher_fail() {
    let mut ctx = common::Context::default();
    let fake_dispatcher = Keypair::new();
    let proof_data = ProofData::new(
        CHAIN_ID,
        vec![IntentHashClaimant::new([1u8; 32].into(), [2u8; 32].into())],
    );

    let result = ctx
        .polymer_prover()
        .prove(&fake_dispatcher, EVM_SOURCE_CHAIN, proof_data);
    assert!(result.is_err_and(common::is_error(
        PolymerProverError::InvalidPortalDispatcher
    )));
}

/// Pins `MAX_INTENTS_PER_PROVE` to what a transaction's log buffer can carry:
/// Solana silently truncates logs past `LOG_MESSAGES_BYTES_LIMIT` (10 KB) while
/// the transaction still succeeds, so a cap above the buffer would hand
/// relayers a success receipt for intents that have no provable log at all.
#[test]
fn prove_via_portal_at_max_intents_emits_every_log_untruncated() {
    let mut ctx = common::Context::default();
    let intents = ctx.fulfill_rand_intents(MAX_INTENTS_PER_PROVE, polymer_prover::ID);
    let intent_hashes: Vec<_> = intents.iter().map(|intent| intent.intent_hash).collect();
    let fulfill_markers: Vec<_> = intent_hashes
        .iter()
        .map(|hash| state::FulfillMarker::pda(hash).0)
        .collect();
    let claimants: Vec<_> = fulfill_markers
        .iter()
        .map(|marker| {
            ctx.account::<state::FulfillMarker>(marker)
                .unwrap()
                .claimant
        })
        .collect();

    let result = ctx
        .portal()
        .prove_intent_via_program_with_compute_limit(
            polymer_prover::ID,
            intent_hashes.clone(),
            EVM_SOURCE_CHAIN,
            fulfill_markers,
            state::dispatcher_pda(&polymer_prover::ID).0,
            vec![],
            vec![],
            MAX_INTENTS_COMPUTE_UNIT_LIMIT,
        )
        .unwrap();

    assert!(
        !result.logs.iter().any(|log| log.contains("truncated")),
        "log buffer overflowed at MAX_INTENTS_PER_PROVE = {MAX_INTENTS_PER_PROVE}\nlogs: {:#?}",
        result.logs
    );

    let prove_lines = result
        .logs
        .iter()
        .filter(|log| log.starts_with("Program log: Prove: program: "))
        .count();
    assert_eq!(
        prove_lines, MAX_INTENTS_PER_PROVE,
        "logs: {:#?}",
        result.logs
    );

    for (intent_hash, claimant) in intent_hashes.into_iter().zip(claimants) {
        let payload = prove_log_payload(
            EVM_SOURCE_CHAIN,
            CHAIN_ID,
            &IntentHashClaimant::new(intent_hash, claimant),
        );
        let expected = format!(
            "Program log: {}",
            prove_log_line(&polymer_prover::ID, &payload)
        );
        assert!(
            result.logs.contains(&expected),
            "missing log line {expected}\nlogs: {:#?}",
            result.logs
        );
    }
}
