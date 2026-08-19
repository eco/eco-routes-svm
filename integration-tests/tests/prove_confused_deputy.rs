//! Regression test for the per-prover dispatcher scoping in `portal::prove`.
//!
//! Security property under test: **a caller-chosen prover program cannot cause a
//! `Proof` to be minted for an intent it did not legitimately prove.** The test
//! drives `portal::prove` through `malicious-prover` (a stand-in prover that
//! re-CPIs local-prover with the inherited dispatcher signer); the assertion
//! holds only when the dispatcher PDA is scoped per-prover.

use anchor_lang::AnchorSerialize;
use eco_svm_std::prover::{IntentHashClaimant, Proof, ProofData, ProveArgs};
use eco_svm_std::{event_authority_pda, Bytes32, CHAIN_ID};
use local_prover::instructions::LocalProverError;
use local_prover::state::ProofAccount;
use portal::state;
use solana_sdk::instruction::AccountMeta;
use solana_sdk::signer::Signer;

pub mod common;

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
