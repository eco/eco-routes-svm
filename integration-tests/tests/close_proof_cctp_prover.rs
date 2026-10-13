use anchor_lang::{InstructionData, ToAccountMetas};
use cctp_prover::instructions::CctpProverError;
use cctp_prover::state::pda_payer_pda;
use eco_svm_std::prover::{CloseProofArgs, IntentHashClaimant, Proof, ProofData};
use portal::events::IntentWithdrawn;
use portal::state::{vault_pda, WithdrawnMarker};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{
    cleanup_tail, evm_sender, peers, BASE_CHAIN_ID, BASE_DOMAIN,
};

pub mod common;

fn initialized() -> common::Context {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.cctp_prover().install(authority.pubkey());
    context.cctp_prover().init(&authority, peers()).unwrap();
    context
}

#[test]
fn close_proof_rejects_foreign_closer() {
    let mut context = initialized();
    let hash = [5u8; 32].into();
    let proof = Proof::pda(&hash, &cctp_prover::ID).0;
    context.set_proof(
        proof,
        Proof::new(BASE_CHAIN_ID, Pubkey::new_unique()),
        cctp_prover::ID,
    );
    let impostor = Keypair::new();
    let ix = Instruction {
        program_id: cctp_prover::ID,
        accounts: cctp_prover::accounts::CloseProof {
            portal_proof_closer: impostor.pubkey(),
            proof,
            pda_payer: pda_payer_pda().0,
        }
        .to_account_metas(None),
        data: cctp_prover::instruction::CloseProof {
            args: CloseProofArgs::new(hash, vec![]),
        }
        .data(),
    };

    let result = context.cctp_prover().send(vec![ix], &[&impostor]);

    assert!(result.is_err_and(common::is_error(CctpProverError::InvalidPortalProofCloser)));
    assert!(context.get_account(&proof).is_some());
}

/// End to end on the source chain: a proof delivered through Circle's real
/// transmitter pays `withdraw`, and Portal's `close_proof` returns its rent to
/// the `pda_payer` that funded it.
#[test]
fn delivered_proof_withdraws_and_cleanup_refunds_pda_payer() {
    let mut context = initialized();
    let (reward, route_hash, hash) = context
        .cctp_prover()
        .funded_native_intent(BASE_CHAIN_ID, cctp_prover::ID);
    let claimant = Pubkey::new_unique();
    context
        .cctp_prover()
        .deliver(
            BASE_DOMAIN,
            evm_sender(0xba),
            1000,
            ProofData::new(
                BASE_CHAIN_ID,
                vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
            ),
        )
        .unwrap();
    let proof = Proof::pda(&hash, &cctp_prover::ID).0;
    let proof_rent = context.get_account(&proof).unwrap().lamports;
    let pda_payer_before = context.balance(&pda_payer_pda().0);

    let result = context.portal().withdraw_intent(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        claimant,
        proof,
        WithdrawnMarker::pda(&hash).0,
        Vec::<AccountMeta>::new(),
        Vec::<AccountMeta>::new(),
    );

    assert!(result.is_ok_and(common::contains_event(IntentWithdrawn::new(hash, claimant))));
    assert_eq!(context.balance(&claimant), reward.native_amount);
    context
        .portal()
        .close_proof(BASE_CHAIN_ID, route_hash, reward, cleanup_tail(&hash))
        .unwrap();
    assert!(context.get_account(&proof).is_none());
    assert_eq!(
        context.balance(&pda_payer_pda().0),
        pda_payer_before + proof_rent
    );
}

#[test]
fn cleanup_rejects_a_recipient_other_than_pda_payer() {
    let mut context = initialized();
    let (reward, route_hash, hash) = context
        .cctp_prover()
        .funded_native_intent(BASE_CHAIN_ID, cctp_prover::ID);
    let proof = Proof::pda(&hash, &cctp_prover::ID).0;
    context.set_proof(
        proof,
        Proof::new(BASE_CHAIN_ID, Pubkey::new_unique()),
        cctp_prover::ID,
    );
    context.set_withdrawn_marker(WithdrawnMarker::pda(&hash).0);
    let payer = context.payer.pubkey();

    let result = context.portal().close_proof(
        BASE_CHAIN_ID,
        route_hash,
        reward,
        vec![
            AccountMeta::new(proof, false),
            AccountMeta::new(payer, true),
        ],
    );

    assert!(result.is_err_and(common::is_error(CctpProverError::InvalidPdaPayer)));
    assert!(context.get_account(&proof).is_some());
}
