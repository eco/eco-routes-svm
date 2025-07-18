use std::iter;

use anchor_lang::prelude::AccountMeta;
use eco_svm_std::prover::Proof;
use eco_svm_std::CHAIN_ID;
use local_prover::instructions::LocalProverError;
use local_prover::state::ProofAccount;
use portal::state;
use portal::types::intent_hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn close_proof_should_succeed() {
    let mut ctx = common::Context::default();
    let mut intent = ctx.rand_intent();
    intent.destination = CHAIN_ID;
    intent.reward.prover = local_prover::ID;
    intent.route.tokens.clear();
    intent.route.calls.clear();
    intent.reward.tokens.clear();
    let route_hash = intent.route.hash();

    let intent_hash = intent_hash(intent.destination, &route_hash, &intent.reward.hash());
    let vault_pda = state::vault_pda(&intent_hash).0;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, intent.reward.native_amount).unwrap();
    ctx.portal()
        .fund_intent(&intent, vault_pda, route_hash, false, vec![])
        .unwrap();

    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

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

    let dispatcher = state::dispatcher_pda().0;
    let proof_pda = Proof::pda(&intent_hash, &local_prover::ID).0;

    ctx.portal()
        .prove_intent_via_local_prover(intent_hash, CHAIN_ID, fulfill_marker, dispatcher, proof_pda)
        .unwrap();

    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let claimant_pubkey = Pubkey::try_from(claimant.as_ref()).unwrap();
    let payer = ctx.payer.pubkey();

    let proof_account = ctx.account::<ProofAccount>(&proof_pda);
    assert!(proof_account.is_some());

    ctx.portal()
        .withdraw_intent(
            &intent,
            vault_pda,
            route_hash,
            claimant_pubkey,
            proof_pda,
            withdrawn_marker,
            state::proof_closer_pda().0,
            vec![],
            iter::once(AccountMeta::new(payer, true)),
        )
        .unwrap();

    let proof_account = ctx.account::<ProofAccount>(&proof_pda);
    assert!(proof_account.is_none());
}

#[test]
fn close_proof_invalid_portal_proof_closer_fail() {
    let mut ctx = common::Context::default();
    let invalid_proof_closer = ctx.payer.insecure_clone();
    let intent_hash = [1u8; 32].into();
    let claimant = ctx.payer.pubkey();
    let destination = CHAIN_ID;
    let proof_pda = Proof::pda(&intent_hash, &local_prover::ID).0;

    ctx.set_proof(
        proof_pda,
        Proof::new(destination, claimant),
        local_prover::ID,
    );

    let result = ctx
        .local_prover()
        .close_proof(&invalid_proof_closer, proof_pda);
    assert!(result.is_err_and(common::is_error(LocalProverError::InvalidPortalProofCloser)));
}
