//! `prove` through Portal, the only caller that can sign `dispatcher_pda`.
//!
//! Reachability note (spec section 6): `check_prove_args`'s `EmptyProofData`
//! and `InvalidDestination` branches cannot be driven from here. Portal rejects
//! an empty batch first (`PortalError::EmptyIntentHashes`, pinned below) and
//! always builds `ProofData::new(CHAIN_ID, ..)`, and a test keypair cannot sign
//! the `portal_dispatcher` PDA to bypass it. Both are defence in depth, pinned
//! by the `check_prove_args` unit tests in
//! `programs/polymer-prover/src/instructions/prove.rs`.

use eco_svm_std::prover::{IntentHashClaimant, ProofData};
use eco_svm_std::CHAIN_ID;
use polymer_prover::instructions::{
    prove_log_line, prove_log_payload, PolymerProverError, MAX_INTENTS_PER_PROVE,
};
use portal::state;
use solana_sdk::signature::Keypair;

use crate::common::polymer_prover_context::PolymerProver;

pub mod common;

const EVM_SOURCE_CHAIN: u64 = 8453;
/// `solana_svm_log_collector`'s `LOG_MESSAGES_BYTES_LIMIT`, which the crate
/// keeps private; mirrored so the headroom assertion names the real budget.
const LOG_MESSAGES_BYTES_LIMIT: usize = 10_000;
/// End-to-end log cost of one intent: the 222-byte `Prove:` line, its 13-byte
/// `Program log: ` prefix, and Portal's 110-byte `Program data:` `IntentProven`
/// event (72 bytes base64-encoded).
const LOG_BYTES_PER_INTENT: usize = 345;

struct ProvenBatch {
    result: litesvm::types::TransactionMetadata,
    expected_lines: Vec<String>,
}

/// Fulfils `count` intents against polymer-prover and proves them through
/// Portal with the full-batch compute limit (Portal loads one `FulfillMarker`
/// per intent and the prover hex-encodes each, well past the 200k default).
/// `expected_lines` are the `Program log: Prove: ...` lines in payload order.
fn prove_batch(count: usize) -> ProvenBatch {
    let mut ctx = common::Context::default();
    let intents = ctx.fulfill_rand_intents(count, polymer_prover::ID);
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
            PolymerProver::VALIDATE_COMPUTE_UNIT_LIMIT,
        )
        .unwrap();

    let expected_lines = intent_hashes
        .iter()
        .zip(&claimants)
        .map(|(hash, claimant)| {
            let payload = prove_log_payload(
                EVM_SOURCE_CHAIN,
                CHAIN_ID,
                &IntentHashClaimant::new(*hash, *claimant),
            );
            format!(
                "Program log: {}",
                prove_log_line(&polymer_prover::ID, &payload)
            )
        })
        .collect();

    ProvenBatch {
        result,
        expected_lines,
    }
}

fn prove_lines(logs: &[String]) -> Vec<&String> {
    logs.iter()
        .filter(|log| log.starts_with("Program log: Prove: program: "))
        .collect()
}

/// One self-contained line per pair, in payload order — the emission order is
/// pinned, not just membership, because the EVM `validateSolana` parser is
/// stateless across lines (spec section 3.5).
#[test]
fn prove_via_portal_emits_one_polymer_log_per_intent() {
    let batch = prove_batch(3);

    assert_eq!(
        prove_lines(&batch.result.logs),
        batch.expected_lines.iter().collect::<Vec<_>>(),
        "logs: {:#?}",
        batch.result.logs
    );
}

/// polymer-prover's logs are the cross-chain ABI: EVM
/// `PolymerProver._processSolanaLog` reverts `InvalidSolanaLog()` on any line
/// without a `program: ` head, and one bad line aborts `validateSolana` for the
/// whole batch. Anchor's dispatcher would emit `Instruction: Prove` before the
/// `Prove:` lines unless `no-log-ix-name` is on, which the crate enables by
/// default. Scoped to polymer-prover's own invocation window: Portal's `prove`
/// legitimately logs its own `Instruction: Prove` at depth 1.
#[test]
fn prove_emits_only_prove_lines_from_polymer_prover() {
    let batch = prove_batch(2);
    let logs = &batch.result.logs;

    let invoke = format!("Program {} invoke [2]", polymer_prover::ID);
    let success = format!("Program {} success", polymer_prover::ID);
    let start = logs
        .iter()
        .position(|log| *log == invoke)
        .expect("polymer invoke");
    let end = logs[start..]
        .iter()
        .position(|log| *log == success)
        .expect("polymer success")
        + start;
    let program_logs: Vec<_> = logs[start + 1..end]
        .iter()
        .filter_map(|line| line.strip_prefix("Program log: "))
        .collect();

    assert_eq!(program_logs.len(), 2, "logs: {logs:#?}");
    for body in program_logs {
        assert!(
            body.starts_with("Prove: program: "),
            "polymer-prover emitted a non-`Prove:` log the EVM parser reverts on: {body}\nlogs: {logs:#?}"
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

/// The empty-batch guard the Portal → polymer-prover path actually hits lives
/// in Portal (see the module comment); mirrors `prove_local_prover.rs`.
#[test]
fn prove_empty_intent_hashes_fail() {
    let mut ctx = common::Context::default();

    let result = ctx.portal().prove_intent_via_program(
        polymer_prover::ID,
        vec![],
        EVM_SOURCE_CHAIN,
        vec![],
        state::dispatcher_pda(&polymer_prover::ID).0,
        vec![],
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::EmptyIntentHashes
    )));
}

/// Pins `MAX_INTENTS_PER_PROVE` to what a transaction's log buffer can carry:
/// Solana silently truncates logs past `LOG_MESSAGES_BYTES_LIMIT` (10 KB) while
/// the transaction still succeeds, so a cap above the buffer would hand
/// relayers a success receipt for intents that have no provable log at all.
#[test]
fn prove_via_portal_at_max_intents_emits_every_log_untruncated() {
    let batch = prove_batch(MAX_INTENTS_PER_PROVE);
    let logs = &batch.result.logs;

    assert!(
        !logs.iter().any(|log| log.contains("truncated")),
        "log buffer overflowed at MAX_INTENTS_PER_PROVE = {MAX_INTENTS_PER_PROVE}\nlogs: {logs:#?}"
    );
    assert_eq!(
        prove_lines(logs).len(),
        MAX_INTENTS_PER_PROVE,
        "logs: {logs:#?}"
    );
    for expected in &batch.expected_lines {
        assert!(
            logs.contains(expected),
            "missing log line {expected}\nlogs: {logs:#?}"
        );
    }

    // The cap is only safe while the batch leaves slack for future Portal log
    // additions; without this, the first signal of an over-budget cap is the
    // truncation assertion above firing at the operating point (spec 3.5).
    // Measured ~8.9 KB at 24, i.e. room for about three more intents.
    let bytes: usize = logs.iter().map(String::len).sum();
    assert!(
        bytes + 2 * LOG_BYTES_PER_INTENT <= LOG_MESSAGES_BYTES_LIMIT,
        "prove at MAX_INTENTS_PER_PROVE = {MAX_INTENTS_PER_PROVE} used {bytes} of \
         {LOG_MESSAGES_BYTES_LIMIT} log bytes, leaving less than the two intents' \
         worth of headroom the cap assumes\nlogs: {logs:#?}"
    );
}

/// `TooManyIntents` wired into the entrypoint, one past the cap. Kept only
/// because it fails with that error and not with compute exhaustion.
#[test]
fn prove_via_portal_above_max_intents_fail() {
    let mut ctx = common::Context::default();
    let intents = ctx.fulfill_rand_intents(MAX_INTENTS_PER_PROVE + 1, polymer_prover::ID);
    let intent_hashes: Vec<_> = intents.iter().map(|intent| intent.intent_hash).collect();
    let fulfill_markers: Vec<_> = intent_hashes
        .iter()
        .map(|hash| state::FulfillMarker::pda(hash).0)
        .collect();

    let result = ctx.portal().prove_intent_via_program_with_compute_limit(
        polymer_prover::ID,
        intent_hashes,
        EVM_SOURCE_CHAIN,
        fulfill_markers,
        state::dispatcher_pda(&polymer_prover::ID).0,
        vec![],
        vec![],
        PolymerProver::VALIDATE_COMPUTE_UNIT_LIMIT,
    );
    assert!(
        result
            .clone()
            .is_err_and(common::is_error(PolymerProverError::TooManyIntents)),
        "{result:?}"
    );
}
