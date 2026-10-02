//! Regression tests for the per-prover dispatcher scoping in `portal::prove`.
//!
//! Security property under test: **a caller-chosen prover program cannot cause a
//! proof to be minted for an intent it did not legitimately prove.** Each arm
//! drives `portal::prove` through `malicious-prover` (a stand-in prover that
//! re-CPIs the real prover named in its tail with the inherited dispatcher
//! signer); the assertions hold only when the dispatcher PDA is scoped
//! per-prover. local-prover mints a `Proof` account; polymer-prover mints a
//! `Prove:` log line, so its arm asserts on logs.

use anchor_lang::AnchorSerialize;
use eco_svm_std::prover::{IntentHashClaimant, Proof, ProofData, ProveArgs};
use eco_svm_std::{event_authority_pda, Bytes32, CHAIN_ID};
use local_prover::instructions::LocalProverError;
use local_prover::state::ProofAccount;
use polymer_prover::instructions::PolymerProverError;
use portal::state;
use solana_sdk::instruction::AccountMeta;
use solana_sdk::signer::Signer;

pub mod common;

/// An EVM source chain, as the relayer would pass through `domain_id`.
const EVM_SOURCE_CHAIN: u64 = 8453;

#[test]
fn malicious_prover_cannot_mint_proof_via_portal_dispatcher() {
    let mut ctx = common::Context::default();

    // Entry cost: the attacker self-fulfills one throwaway intent so a valid
    // `FulfillMarker` exists to pass `portal::prove`'s marker check.
    let attacker_intent_hash = ctx.fulfill_rand_intents(1, local_prover::ID)[0].intent_hash;
    let attacker_marker = state::FulfillMarker::pda(&attacker_intent_hash).0;

    // The victim: an intent the attacker never fulfilled. A `Proof` minted here
    // with an attacker-chosen claimant is the theft primitive.
    let victim_intent_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let attacker_claimant: Bytes32 = ctx.payer.pubkey().to_bytes().into();
    let victim_proof = Proof::pda(&victim_intent_hash, &local_prover::ID).0;

    // Attacker payload smuggled through `portal::prove`'s verbatim `data` field:
    // a local-prover `ProveArgs` naming the victim intent and attacker claimant.
    let malicious_data = ProveArgs::new(
        CHAIN_ID,
        ProofData::new(
            CHAIN_ID,
            vec![IntentHashClaimant::new(
                victim_intent_hash,
                attacker_claimant,
            )],
        ),
        vec![],
    );
    let mut data = Vec::new();
    malicious_data.serialize(&mut data).unwrap();

    // Accounts the malicious intermediary needs to re-CPI the real local-prover,
    // supplied as the `prove_accounts` tail (everything after the fulfill marker).
    // Order matches `eco_svm_std::prover::prove`'s fixed layout, which
    // `malicious-prover` mirrors: after the inherited dispatcher signer come
    // payer, system_program, event_authority and the callee, then a tail of the
    // real local-prover and the proofs to mint.
    let prove_accounts = vec![
        AccountMeta::new(ctx.payer.pubkey(), true),
        AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
        AccountMeta::new_readonly(event_authority_pda(&local_prover::ID).0, false),
        AccountMeta::new_readonly(malicious_prover::ID, false),
        AccountMeta::new_readonly(local_prover::ID, false),
        AccountMeta::new(victim_proof, false),
    ];

    let result = ctx.portal().prove_intent_via_program(
        malicious_prover::ID,
        vec![attacker_intent_hash],
        CHAIN_ID,
        vec![attacker_marker],
        state::dispatcher_pda(&malicious_prover::ID).0,
        data,
        prove_accounts,
    );

    // The exploit must be rejected at local-prover's caller gate — not merely
    // fail — and no victim proof may exist.
    assert!(result.is_err_and(common::is_error(LocalProverError::InvalidCaller)));
    assert!(ctx.account::<ProofAccount>(&victim_proof).is_none());
}

/// Same property against polymer-prover: its `prove` accepts only
/// `dispatcher_pda(&polymer_prover::ID)`. polymer-prover mints no `Proof`; the
/// `Prove: program: <id>, <160 hex>` line it logs *is* the proof the EVM
/// `PolymerProver.validateSolana` consumes, so a forged line for a victim
/// intent with an attacker claimant would be the theft primitive, and the
/// negative assertion is on the transaction logs rather than on an account.
#[test]
fn malicious_prover_cannot_emit_polymer_prove_log_via_portal_dispatcher() {
    let mut ctx = common::Context::default();

    // Entry cost: one throwaway intent the attacker fulfills, so `portal::prove`
    // finds a valid `FulfillMarker`.
    let attacker_intent_hash = ctx.fulfill_rand_intents(1, polymer_prover::ID)[0].intent_hash;
    let attacker_marker = state::FulfillMarker::pda(&attacker_intent_hash).0;

    let victim_intent_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let attacker_claimant: Bytes32 = ctx.payer.pubkey().to_bytes().into();
    let victim_pair = IntentHashClaimant::new(victim_intent_hash, attacker_claimant);

    // Smuggled through `portal::prove`'s verbatim `data`: a polymer-prover
    // `ProveArgs` naming the victim intent and the attacker's claimant. Every
    // field is otherwise valid — `check_prove_args` requires
    // `proof_data.destination == CHAIN_ID` — so the dispatcher gate is the only
    // thing that can reject this.
    let malicious_data = ProveArgs::new(
        EVM_SOURCE_CHAIN,
        ProofData::new(CHAIN_ID, vec![victim_pair.clone()]),
        vec![],
    );
    let mut data = Vec::new();
    malicious_data.serialize(&mut data).unwrap();

    // `malicious-prover`'s fixed `Accounts` shape: payer, system_program,
    // event_authority, this_program, then the nested CPI target. polymer-prover's
    // `prove` takes a single account and mints nothing, so the event authority is
    // an unused filler slot and the tail carries no proof accounts. Specifically
    // no `dispatcher_pda(&polymer_prover::ID)` is supplied: malicious-prover can
    // only forward slot 0, `dispatcher_pda(&malicious_prover::ID)`, which is
    // exactly the credential under test.
    let prove_accounts = vec![
        AccountMeta::new(ctx.payer.pubkey(), true),
        AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
        AccountMeta::new_readonly(event_authority_pda(&polymer_prover::ID).0, false),
        AccountMeta::new_readonly(malicious_prover::ID, false),
        AccountMeta::new_readonly(polymer_prover::ID, false),
    ];

    let result = ctx.portal().prove_intent_via_program(
        malicious_prover::ID,
        vec![attacker_intent_hash],
        EVM_SOURCE_CHAIN,
        vec![attacker_marker],
        state::dispatcher_pda(&malicious_prover::ID).0,
        data,
        prove_accounts,
    );

    // The CPI chain must actually reach polymer-prover, and polymer-prover must
    // be the program that rejects it: a bare `Custom(6000)` is ambiguous across
    // programs.
    assert!(result
        .clone()
        .is_err_and(common::reached_program(polymer_prover::ID)));
    assert!(result.clone().is_err_and(common::is_program_error(
        polymer_prover::ID,
        PolymerProverError::InvalidPortalDispatcher
    )));
    // And no provable line escaped: neither the exact forged line nor any
    // `Prove:` line at all.
    let logs = result.unwrap_err().meta.logs;
    let forged = format!(
        "Program log: {}",
        polymer_prover::instructions::prove_log_line(
            &polymer_prover::ID,
            &polymer_prover::instructions::prove_log_payload(
                EVM_SOURCE_CHAIN,
                CHAIN_ID,
                &victim_pair
            ),
        )
    );
    assert!(!logs.contains(&forged), "logs: {logs:#?}");
    assert!(
        !logs
            .iter()
            .any(|log| log.starts_with("Program log: Prove: program: ")),
        "logs: {logs:#?}"
    );
}
