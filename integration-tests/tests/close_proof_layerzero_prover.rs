use std::iter;

use anchor_lang::{Discriminator, InstructionData, ToAccountMetas};
use eco_svm_std::prover::Proof;
use layerzero_prover::instructions::LayerZeroProverError;
use layerzero_prover::layerzero;
use layerzero_prover::state::pda_payer_pda;
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
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
        data: layerzero_prover::instruction::CloseProof {}.data(),
    };
    let result = context
        .layerzero_prover()
        .send(vec![instruction], &[&impostor]);

    assert!(result.is_err_and(common::is_error(
        LayerZeroProverError::InvalidPortalProofCloser
    )));
}

#[test]
fn withdraw_closes_proof_and_refunds_pda_payer() {
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
        proof_closer_pda(&layerzero_prover::ID).0,
        Vec::<AccountMeta>::new(),
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );

    assert!(result.is_ok());
    assert!(context.get_account(&proof).is_none());
    assert_eq!(
        context.balance(&pda_payer_pda().0),
        pda_payer_before + proof_rent
    );
    assert_eq!(context.balance(&claimant), reward.native_amount);
}
