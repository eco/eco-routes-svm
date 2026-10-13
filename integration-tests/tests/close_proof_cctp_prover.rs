use anchor_lang::{InstructionData, ToAccountMetas};
use cctp_prover::cctp;
use cctp_prover::instructions::CctpProverError;
use eco_svm_std::prover::{CloseProofArgs, IntentHashClaimant, Proof, ProofData};
use eco_svm_std::{claimant, Bytes32};
use portal::events::IntentWithdrawn;
use portal::state::{vault_pda, WithdrawnMarker};
use portal::types::Reward;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{
    cleanup_tail, evm_sender, peers, proof_pdas, BASE_CHAIN_ID, BASE_DOMAIN,
};

pub mod common;

fn initialized() -> common::Context {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.cctp_prover().install(authority.pubkey());
    context.cctp_prover().init(&authority, peers()).unwrap();
    context
}

/// A funded intent naming cctp-prover with a planted proof for `claimant`.
fn proven_intent(
    context: &mut common::Context,
    claimant: Pubkey,
) -> (Reward, Bytes32, Bytes32, Pubkey) {
    let (reward, route_hash, hash) = context
        .cctp_prover()
        .funded_native_intent(BASE_CHAIN_ID, cctp_prover::ID);
    let proof = Proof::pda(&hash, &cctp_prover::ID).0;
    context.set_proof(proof, Proof::new(BASE_CHAIN_ID, claimant), cctp_prover::ID);

    (reward, route_hash, hash, proof)
}

#[test]
fn close_proof_rejects_foreign_closer() {
    let mut context = initialized();
    let hash = [5u8; 32].into();
    let proof = Proof::pda(&hash, &cctp_prover::ID).0;
    let claimant = Pubkey::new_unique();
    context.set_proof(proof, Proof::new(BASE_CHAIN_ID, claimant), cctp_prover::ID);
    let impostor = Keypair::new();
    let ix = Instruction {
        program_id: cctp_prover::ID,
        accounts: cctp_prover::accounts::CloseProof {
            portal_proof_closer: impostor.pubkey(),
            proof,
            rent_recipient: claimant,
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

/// End to end on the source chain, with a relayer that is not the claimant:
/// the relayer pays the rent of a proof delivered through Circle's real
/// transmitter, the proof pays `withdraw`, and Portal's `close_proof` refunds
/// that rent to the claimant, not the relayer.
#[test]
fn relayer_pays_rent_and_cleanup_refunds_claimant() {
    let mut context = initialized();
    let (reward, route_hash, hash) = context
        .cctp_prover()
        .funded_native_intent(BASE_CHAIN_ID, cctp_prover::ID);
    let claimant = Pubkey::new_unique();
    let relayer = Keypair::new();
    context
        .airdrop(&relayer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let data = ProofData::new(
        BASE_CHAIN_ID,
        vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
    );
    let proofs = proof_pdas(&data);
    let inbound = context
        .cctp_prover()
        .inbound(BASE_DOMAIN, evm_sender(0xba), 1000, data);
    let relayer_before = context.balance(&relayer.pubkey());

    context
        .cctp_prover()
        .receive_by(&relayer, &inbound, &proofs)
        .unwrap();

    let proof = Proof::pda(&hash, &cctp_prover::ID).0;
    let proof_rent = context.get_account(&proof).unwrap().lamports;
    let nonce = cctp::nonce(&inbound.message).unwrap();
    let used_nonce_rent = context
        .get_account(&cctp::used_nonce_pda(&nonce).0)
        .unwrap()
        .lamports;
    let relayer_after_delivery = context.balance(&relayer.pubkey());
    assert_eq!(
        relayer_before - relayer_after_delivery,
        used_nonce_rent + proof_rent
    );

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
    let payer = context.payer.pubkey();
    context
        .portal()
        .close_proof(
            BASE_CHAIN_ID,
            route_hash,
            reward.clone(),
            cleanup_tail(&hash, claimant, payer),
        )
        .unwrap();
    assert!(context.get_account(&proof).is_none());
    assert_eq!(
        context.balance(&claimant),
        reward.native_amount + proof_rent
    );
    assert_eq!(context.balance(&relayer.pubkey()), relayer_after_delivery);
}

/// A non-signing claimant receives exactly the proof's rent.
#[test]
fn cleanup_refunds_exactly_the_rent_to_the_claimant() {
    let mut context = initialized();
    let claimant = Pubkey::new_unique();
    let (reward, route_hash, hash, proof) = proven_intent(&mut context, claimant);
    context.set_withdrawn_marker(WithdrawnMarker::pda(&hash).0);
    let proof_rent = context.get_account(&proof).unwrap().lamports;
    let payer = context.payer.pubkey();
    let tail = cleanup_tail(&hash, claimant, payer);
    assert!(!tail[1].is_signer);

    context
        .portal()
        .close_proof(BASE_CHAIN_ID, route_hash, reward, tail)
        .unwrap();

    assert!(context.get_account(&proof).is_none());
    assert_eq!(context.balance(&claimant), proof_rent);
}

/// On a payable proof only the claimant can take the rent, signer or not.
#[test]
fn cleanup_rejects_a_recipient_other_than_the_claimant() {
    let mut context = initialized();
    let (reward, route_hash, hash, proof) = proven_intent(&mut context, Pubkey::new_unique());
    context.set_withdrawn_marker(WithdrawnMarker::pda(&hash).0);
    let payer = context.payer.pubkey();

    for recipient in [
        AccountMeta::new(payer, true),
        AccountMeta::new(Pubkey::new_unique(), false),
    ] {
        let result = context.portal().close_proof(
            BASE_CHAIN_ID,
            route_hash,
            reward.clone(),
            vec![AccountMeta::new(proof, false), recipient],
        );

        assert!(result.is_err_and(common::is_program_error(
            cctp_prover::ID,
            CctpProverError::InvalidRentRecipient
        )));
        assert!(context.get_account(&proof).is_some());
    }
}

/// A cancellation's claimant is the uncontrolled sentinel, so its rent goes to
/// the closer, who must sign: Portal forwards the tail's signer flag, so
/// nobody can name an account they do not control.
#[test]
fn cancelled_cleanup_pays_only_a_signing_closer() {
    let mut context = initialized();
    let (reward, route_hash, hash, proof) = proven_intent(&mut context, claimant::cancelled());
    context.warp_to_timestamp(reward.deadline.try_into().unwrap());
    let proof_rent = context.get_account(&proof).unwrap().lamports;

    for recipient in [Pubkey::new_unique(), claimant::cancelled()] {
        let result = context.portal().close_proof(
            BASE_CHAIN_ID,
            route_hash,
            reward.clone(),
            vec![
                AccountMeta::new(proof, false),
                AccountMeta::new(recipient, false),
            ],
        );

        assert!(result.is_err_and(common::is_program_error(
            cctp_prover::ID,
            CctpProverError::InvalidRentRecipient
        )));
        assert!(context.get_account(&proof).is_some());
    }

    let closer = Keypair::new();
    let ix = context.portal().close_proof_instruction(
        BASE_CHAIN_ID,
        route_hash,
        reward,
        cleanup_tail(&hash, claimant::cancelled(), closer.pubkey()),
    );
    context.cctp_prover().send(vec![ix], &[&closer]).unwrap();

    assert!(context.get_account(&proof).is_none());
    assert_eq!(context.balance(&closer.pubkey()), proof_rent);
}
