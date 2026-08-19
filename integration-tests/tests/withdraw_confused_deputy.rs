//! Regression test for the per-prover proof_closer scoping in `portal::withdraw`.
//!
//! Security property under test: **a caller-chosen prover cannot cause an
//! unrelated proof account to be closed.** `portal::withdraw` CPIs the prover's
//! `close_proof` with the portal `proof_closer` PDA as an inherited signer and
//! forwards the withdraw call's remaining accounts. The test drives it through
//! `malicious-proof-closer` (a stand-in prover whose `close_proof` re-CPIs the real
//! local-prover to close a proof it does not own); the assertion holds only when
//! the `proof_closer` PDA is scoped per-prover.

use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CHAIN_ID};
use local_prover::instructions::LocalProverError;
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use portal::types::{intent_hash, Reward};
use solana_sdk::instruction::AccountMeta;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn malicious_proof_closer_cannot_close_unrelated_proof_via_proof_closer() {
    let mut ctx = common::Context::default();

    // A victim's legitimate local-prover proof, unrelated to the attacker.
    let victim_intent_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let victim_claimant = Pubkey::new_unique();
    let victim_proof = Proof::pda(&victim_intent_hash, &local_prover::ID).0;
    ctx.set_proof(
        victim_proof,
        Proof::new(CHAIN_ID, victim_claimant),
        local_prover::ID,
    );

    // The attacker's own intent, with `reward.prover` set to the malicious
    // program. Zero tokens/native so every remaining account flows to the
    // `close_proof` CPI.
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

    // A proof under the attacker's own prover that passes `validate_proof`.
    let attacker_proof = Proof::pda(&attacker_intent_hash, &malicious_proof_closer::ID).0;
    ctx.set_proof(
        attacker_proof,
        Proof::new(CHAIN_ID, attacker),
        malicious_proof_closer::ID,
    );

    let result = ctx.portal().withdraw_intent(
        CHAIN_ID,
        reward,
        vault,
        route_hash,
        attacker,
        attacker_proof,
        WithdrawnMarker::pda(&attacker_intent_hash).0,
        proof_closer_pda(&malicious_proof_closer::ID).0,
        vec![],
        vec![
            AccountMeta::new_readonly(local_prover::ID, false),
            AccountMeta::new(victim_proof, false),
            AccountMeta::new(attacker, true),
        ],
    );

    // The CPI chain must actually reach local-prover — otherwise the assertion
    // below would also hold for an exploit that never got off the ground — and
    // local-prover must be the program that rejects it. A bare error code is
    // ambiguous across programs: `InvalidPortalProofCloser` and
    // `PortalError::InvalidMint` are both 6003.
    assert!(result
        .clone()
        .is_err_and(common::reached_program(local_prover::ID)));
    assert!(result.is_err_and(common::is_program_error(
        local_prover::ID,
        LocalProverError::InvalidPortalProofCloser
    )));
}
