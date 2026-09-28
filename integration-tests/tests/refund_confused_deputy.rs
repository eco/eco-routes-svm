//! Regression test for the per-prover proof_closer scoping in `portal::refund`.
//!
//! Security property under test: **a caller-chosen prover cannot cause an
//! unrelated proof account to be closed.** Refunding a proven cancellation
//! CPIs `reward.prover`'s `close_proof` with the portal `proof_closer` PDA as
//! an inherited signer and forwards the refund call's trailing accounts. The
//! test drives it through `malicious-proof-closer` (a stand-in prover whose
//! `close_proof` re-CPIs a real prover to close a proof it does not own); the
//! assertion holds only when the `proof_closer` PDA is scoped per-prover.

use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};
use local_prover::instructions::LocalProverError;
use polymer_prover::instructions::PolymerProverError;
use portal::state::{vault_pda, WithdrawnMarker};
use portal::types::{intent_hash, Reward};
use solana_sdk::instruction::AccountMeta;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

/// Plants `victim_prover`'s proof for an unrelated intent and a `CANCELLED`
/// proof for the attacker's own intent under `malicious-proof-closer`, then
/// refunds the attacker's intent with a tail aimed at the victim's proof.
/// Returns the refund result and the victim's proof address.
fn refund_targeting_victim_proof(
    ctx: &mut common::Context,
    victim_prover: Pubkey,
) -> (common::TransactionResult, Pubkey) {
    let victim_intent_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let victim_proof = Proof::pda(&victim_intent_hash, &victim_prover).0;
    ctx.set_proof(
        victim_proof,
        Proof::new(CHAIN_ID, Pubkey::new_unique()),
        victim_prover,
    );

    // Zero tokens so every trailing account flows to the `close_proof` CPI.
    let attacker = ctx.payer.pubkey();
    let route_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let reward = Reward {
        deadline: ctx.now() + 3600,
        creator: attacker,
        prover: malicious_proof_closer::ID,
        native_amount: 0,
        tokens: vec![],
    };
    let attacker_intent_hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = vault_pda(&attacker_intent_hash).0;
    ctx.airdrop(&vault, 1_000_000_000).unwrap();

    // A cancellation under the attacker's own prover, which `refund` accepts
    // before `reward.deadline`.
    let attacker_proof = Proof::pda(&attacker_intent_hash, &malicious_proof_closer::ID).0;
    ctx.set_proof(
        attacker_proof,
        Proof::new(CHAIN_ID, Pubkey::new_from_array(CANCELLED.into())),
        malicious_proof_closer::ID,
    );

    let result = ctx.portal().refund_intent_with_close_proof(
        CHAIN_ID,
        reward,
        vault,
        route_hash,
        attacker_proof,
        WithdrawnMarker::pda(&attacker_intent_hash).0,
        attacker,
        vec![],
        vec![
            AccountMeta::new_readonly(victim_prover, false),
            AccountMeta::new(victim_proof, false),
            AccountMeta::new(attacker, true),
        ],
    );

    (result, victim_proof)
}

#[test]
fn malicious_proof_closer_cannot_close_unrelated_proof_via_refund() {
    let mut ctx = common::Context::default();

    let (result, victim_proof) = refund_targeting_victim_proof(&mut ctx, local_prover::ID);

    // The CPI chain must actually reach local-prover — otherwise the assertion
    // below would also hold for an exploit that never got off the ground — and
    // local-prover must be the program that rejects it.
    assert!(result
        .clone()
        .is_err_and(common::reached_program(local_prover::ID)));
    assert!(result.is_err_and(common::is_program_error(
        local_prover::ID,
        LocalProverError::InvalidPortalProofCloser
    )));
    assert!(ctx.get_account(&victim_proof).is_some());
}

/// Same property against polymer-prover: its `close_proof` accepts only
/// `proof_closer_pda(&polymer_prover::ID)`.
#[test]
fn malicious_proof_closer_cannot_close_polymer_proof_via_refund() {
    let mut ctx = common::Context::default();

    let (result, victim_proof) = refund_targeting_victim_proof(&mut ctx, polymer_prover::ID);

    assert!(result
        .clone()
        .is_err_and(common::reached_program(polymer_prover::ID)));
    assert!(result.is_err_and(common::is_program_error(
        polymer_prover::ID,
        PolymerProverError::InvalidPortalProofCloser
    )));
    assert!(ctx.get_account(&victim_proof).is_some());
}
