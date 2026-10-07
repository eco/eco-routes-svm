use anchor_lang::{Discriminator, InstructionData, ToAccountMetas};
use eco_svm_std::prover::{CloseProofArgs, Proof};
use layerzero_prover::instructions::LayerZeroProverError;
use layerzero_prover::layerzero;
use layerzero_prover::state::pda_payer_pda;
use portal::events::IntentWithdrawn;
use portal::instructions::PortalError;
use portal::state::{vault_pda, WithdrawnMarker};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::BASE_CHAIN_ID;

pub mod common;

/// Our hand-written endpoint discriminators equal the Anchor-derived ones of
/// the identically named mock instructions (both `sha256("global:<name>")`),
/// and `lz_receive` equals the constant LayerZero's executor hard-codes.
#[test]
fn mirrored_discriminators_match_anchor_derivation() {
    use mock_layerzero_endpoint::instruction as mock;
    let pairs: [(&[u8], [u8; 8]); 10] = [
        (
            mock::RegisterOapp::DISCRIMINATOR,
            layerzero::REGISTER_OAPP_DISCRIMINATOR,
        ),
        (
            mock::InitNonce::DISCRIMINATOR,
            layerzero::INIT_NONCE_DISCRIMINATOR,
        ),
        (
            mock::InitSendLibrary::DISCRIMINATOR,
            layerzero::INIT_SEND_LIBRARY_DISCRIMINATOR,
        ),
        (
            mock::InitReceiveLibrary::DISCRIMINATOR,
            layerzero::INIT_RECEIVE_LIBRARY_DISCRIMINATOR,
        ),
        (
            mock::SetSendLibrary::DISCRIMINATOR,
            layerzero::SET_SEND_LIBRARY_DISCRIMINATOR,
        ),
        (
            mock::SetReceiveLibrary::DISCRIMINATOR,
            layerzero::SET_RECEIVE_LIBRARY_DISCRIMINATOR,
        ),
        (
            mock::InitConfig::DISCRIMINATOR,
            layerzero::INIT_CONFIG_DISCRIMINATOR,
        ),
        (
            mock::SetConfig::DISCRIMINATOR,
            layerzero::SET_CONFIG_DISCRIMINATOR,
        ),
        (mock::Clear::DISCRIMINATOR, layerzero::CLEAR_DISCRIMINATOR),
        (mock::Quote::DISCRIMINATOR, layerzero::QUOTE_DISCRIMINATOR),
    ];
    pairs
        .iter()
        .for_each(|(anchor, mirrored)| assert_eq!(*anchor, mirrored.as_slice()));
}

#[test]
fn close_proof_rejects_foreign_closer() {
    let mut context = common::Context::default();
    context.layerzero_prover().install(Pubkey::new_unique());
    let hash = [5u8; 32].into();
    let proof = Proof::pda(&hash, &layerzero_prover::ID).0;
    context.set_proof(
        proof,
        Proof::new(BASE_CHAIN_ID, Pubkey::new_unique()),
        layerzero_prover::ID,
    );
    let impostor = Keypair::new();

    let instruction = Instruction {
        program_id: layerzero_prover::ID,
        accounts: layerzero_prover::accounts::CloseProof {
            portal_proof_closer: impostor.pubkey(),
            proof,
            pda_payer: pda_payer_pda().0,
        }
        .to_account_metas(None),
        data: layerzero_prover::instruction::CloseProof {
            args: CloseProofArgs::new(hash, vec![]),
        }
        .data(),
    };
    let result = context
        .layerzero_prover()
        .send(vec![instruction], &[&impostor]);

    assert!(result.is_err_and(common::is_error(
        LayerZeroProverError::InvalidPortalProofCloser
    )));
    assert!(context.get_account(&proof).is_some());
}

/// `withdraw` only queries the proof (tail `[proof]`); the separate Portal
/// `close_proof` (tail `[proof, pda_payer]`) returns its rent to the reserve
/// that paid it in `lz_receive`.
#[test]
fn withdraw_leaves_proof_and_cleanup_refunds_pda_payer() {
    let mut context = common::Context::default();
    context.layerzero_prover().install(Pubkey::new_unique());
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let claimant = Pubkey::new_unique();
    let proof = Proof::pda(&hash, &layerzero_prover::ID).0;
    context.set_proof(
        proof,
        Proof::new(BASE_CHAIN_ID, claimant),
        layerzero_prover::ID,
    );
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
    assert!(context.get_account(&proof).is_some());

    context
        .portal()
        .close_proof(
            BASE_CHAIN_ID,
            route_hash,
            reward,
            cleanup_tail(proof, pda_payer_pda().0),
        )
        .unwrap();

    assert!(context.get_account(&proof).is_none());
    assert_eq!(
        context.balance(&pda_payer_pda().0),
        pda_payer_before + proof_rent
    );
}

/// The rent recipient is pinned to `pda_payer`, like hyper-prover's PDA payer:
/// a cleanup that names anyone else fails and leaves the proof in place.
#[test]
fn cleanup_rejects_a_recipient_other_than_pda_payer() {
    let mut context = common::Context::default();
    context.layerzero_prover().install(Pubkey::new_unique());
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let proof = Proof::pda(&hash, &layerzero_prover::ID).0;
    context.set_proof(
        proof,
        Proof::new(BASE_CHAIN_ID, Pubkey::new_unique()),
        layerzero_prover::ID,
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

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidPdaPayer)));
    assert!(context.get_account(&proof).is_some());
}

/// Without a withdrawal, cleanup needs a cancellation at/after the deadline:
/// Portal reaches our `get_proof` through the same tail and refuses a payable
/// proof.
#[test]
fn cleanup_without_withdrawal_refuses_a_payable_proof() {
    let mut context = common::Context::default();
    context.layerzero_prover().install(Pubkey::new_unique());
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let proof = Proof::pda(&hash, &layerzero_prover::ID).0;
    context.set_proof(
        proof,
        Proof::new(BASE_CHAIN_ID, Pubkey::new_unique()),
        layerzero_prover::ID,
    );
    context.warp_to_timestamp(reward.deadline.try_into().unwrap());

    let result = context.portal().close_proof(
        BASE_CHAIN_ID,
        route_hash,
        reward,
        cleanup_tail(proof, pda_payer_pda().0),
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentNotCancelled)));
    assert!(context.get_account(&proof).is_some());
}

fn cleanup_tail(proof: Pubkey, pda_payer: Pubkey) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(proof, false),
        AccountMeta::new(pda_payer, false),
    ]
}

#[test]
fn lz_receive_discriminator_matches_executor_constant() {
    assert_eq!(
        layerzero_prover::instruction::LzReceive::DISCRIMINATOR,
        layerzero::LZ_RECEIVE_DISCRIMINATOR.as_slice()
    );
    // Endpoint `send` is mocked under a custom name with LayerZero's selector.
    assert_eq!(
        mock_layerzero_endpoint::instruction::SendPacket::DISCRIMINATOR,
        layerzero::SEND_DISCRIMINATOR.as_slice()
    );
}
