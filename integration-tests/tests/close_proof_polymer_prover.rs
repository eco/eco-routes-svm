use std::iter;

use anchor_lang::Discriminator;
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CHAIN_ID};
use polymer_prover::instructions::PolymerProverError;
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use portal::types::{intent_hash, Reward};
use solana_sdk::instruction::AccountMeta;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

/// LiteSVM's default fee structure charges 5000 lamports per signature; the
/// withdraw transaction carries exactly one (the payer's).
const TRANSACTION_FEE: u64 = 5_000;

#[test]
fn close_proof_invalid_portal_proof_closer_fail() {
    let mut ctx = common::Context::default();
    let invalid_proof_closer = ctx.payer.insecure_clone();
    let intent_hash = [1u8; 32].into();
    let proof_pda = Proof::pda(&intent_hash, &polymer_prover::ID).0;
    ctx.set_proof(
        proof_pda,
        Proof::new(CHAIN_ID, ctx.payer.pubkey()),
        polymer_prover::ID,
    );

    let result = ctx
        .polymer_prover()
        .close_proof(&invalid_proof_closer, proof_pda);
    assert!(result.is_err_and(common::is_error(
        PolymerProverError::InvalidPortalProofCloser
    )));
}

/// Portal `withdraw` → polymer-prover `close_proof`: the proof is gone and its
/// rent came back to the withdraw payer.
#[test]
fn withdraw_closes_polymer_proof_and_refunds_payer() {
    let mut ctx = common::Context::default();
    let route_hash: Bytes32 = rand::random::<[u8; 32]>().into();
    let reward = Reward {
        deadline: ctx.now() + 3600,
        creator: ctx.creator.pubkey(),
        prover: polymer_prover::ID,
        native_amount: 0,
        tokens: vec![],
    };
    let hash = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = vault_pda(&hash).0;
    ctx.airdrop(&vault, 1_000_000_000).unwrap();
    let claimant = Pubkey::new_unique();
    let proof = Proof::pda(&hash, &polymer_prover::ID).0;
    ctx.set_proof(proof, Proof::new(CHAIN_ID, claimant), polymer_prover::ID);
    let proof_rent = ctx.balance(&proof);
    let payer = ctx.payer.pubkey();
    let payer_before = ctx.balance(&payer);

    let result = ctx.portal().withdraw_intent(
        CHAIN_ID,
        reward,
        vault,
        route_hash,
        claimant,
        proof,
        WithdrawnMarker::pda(&hash).0,
        proof_closer_pda(&polymer_prover::ID).0,
        vec![],
        iter::once(AccountMeta::new(payer, true)),
    );
    assert!(result.is_ok(), "{result:?}");

    assert!(ctx.get_account(&proof).is_none());
    // Payer received the proof rent, net of the withdrawn-marker rent and the
    // single-signature fee it paid.
    let marker_rent = ctx.balance(&WithdrawnMarker::pda(&hash).0);
    assert_eq!(
        ctx.balance(&payer) + marker_rent + TRANSACTION_FEE,
        payer_before + proof_rent
    );
}

/// `Context::set_proof` fabricates every prover's Proof with hyper-prover's
/// `ProofAccount::DISCRIMINATOR`. That stands in for a Proof polymer-prover
/// (or local-prover) actually wrote only because Anchor derives the
/// discriminator from the struct *name* and all three crates call theirs
/// `ProofAccount`; `close_proof`'s `Account<ProofAccount>` is what would reject
/// a mismatch. Pin the coincidence so a rename fails here, with a message
/// naming the cause, rather than as an opaque mismatch inside `withdraw`.
#[test]
fn set_proof_discriminator_matches_every_prover() {
    assert_eq!(
        polymer_prover::state::ProofAccount::DISCRIMINATOR,
        hyper_prover::state::ProofAccount::DISCRIMINATOR
    );
    assert_eq!(
        local_prover::state::ProofAccount::DISCRIMINATOR,
        hyper_prover::state::ProofAccount::DISCRIMINATOR
    );
}
