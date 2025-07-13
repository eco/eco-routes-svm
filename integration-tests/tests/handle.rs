use std::iter;

use anchor_lang::error::ErrorCode;
use anchor_lang::prelude::AccountMeta;
use anchor_lang::system_program;
use eco_svm_std::prover::{self, Proof};
use eco_svm_std::{Bytes32, CHAIN_ID};
use hyper_prover::instructions::HyperProverError;
use hyper_prover::state::{pda_payer_pda, Config, ProofAccount};
use portal::events::IntentWithdrawn;
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use portal::types::intent_hash;
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

use crate::common::sol_amount;

pub mod common;

fn setup() -> common::Context {
    let mut ctx = common::Context::default();

    let pda_payer = pda_payer_pda().0;
    ctx.airdrop(&pda_payer, sol_amount(10.0)).unwrap();

    let sender = ctx.sender.pubkey();
    ctx.hyper_prover()
        .init(vec![sender.to_bytes().into()], Config::pda().0)
        .unwrap();

    ctx
}

fn create_hyperlane_message(
    sender: Bytes32,
    origin: u32,
    destination: u32,
    recipient: Bytes32,
    body: Vec<u8>,
) -> Vec<u8> {
    let mut message = Vec::new();

    // Version (1 byte) - must be 3 for Hyperlane mailbox compatibility
    message.push(3);
    // Nonce (4 bytes)
    message.extend_from_slice(random::<[u8; 4]>().as_slice());
    // Origin domain (4 bytes)
    message.extend_from_slice(&origin.to_be_bytes());
    // Sender (32 bytes) - use a dummy sender
    message.extend_from_slice(sender.as_slice());
    // Destination domain (4 bytes)
    message.extend_from_slice(&destination.to_be_bytes());
    // Recipient (32 bytes)
    message.extend_from_slice(recipient.as_slice());
    // Message body
    message.extend_from_slice(&body);

    message
}

#[test]
fn handle_success() {
    let mut ctx = setup();
    let destination = random();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let intent_hash: Bytes32 = random::<[u8; 32]>().into();
    let payload = claimant
        .iter()
        .copied()
        .chain(intent_hash.to_vec())
        .collect::<Vec<_>>();
    let message = create_hyperlane_message(
        ctx.sender.pubkey().to_bytes().into(),
        destination,
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );
    let pda_payer_pda = pda_payer_pda().0;
    let pda_payer_balance = ctx.balance(&pda_payer_pda);

    let sender = ctx.sender.pubkey();
    let handle_account_metas =
        ctx.hyper_prover()
            .handle_account_metas(destination, sender.to_bytes(), payload);
    let result = ctx.hyperlane().inbox_process(message, handle_account_metas);
    assert!(
        result.is_ok_and(common::contains_cpi_event(prover::IntentFulfilled::new(
            intent_hash,
            claimant
        ),))
    );

    let proof_pda = Proof::pda(&intent_hash, &hyper_prover::ID).0;
    let proof: ProofAccount = ctx.account(&proof_pda).unwrap();
    assert_eq!(destination as u64, proof.0.destination);
    assert_eq!(claimant, proof.0.claimant);
    assert!(pda_payer_balance > ctx.balance(&pda_payer_pda));
}

#[test]
fn handle_withdraw_success() {
    let mut ctx = setup();
    let mut intent = ctx.rand_intent();
    intent.reward.tokens.clear();
    intent.reward.native_amount = 0;
    let claimant = Pubkey::new_unique();
    let intent_hash = intent_hash(
        intent.destination,
        &intent.route.hash(),
        &intent.reward.hash(),
    );
    let payload = claimant
        .as_array()
        .iter()
        .copied()
        .chain(intent_hash.to_vec())
        .collect::<Vec<_>>();
    let message = create_hyperlane_message(
        ctx.sender.pubkey().to_bytes().into(),
        intent.destination.try_into().unwrap(),
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );
    let vault = vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &hyper_prover::ID).0;
    let withdrawn_marker = WithdrawnMarker::pda(&intent_hash).0;
    let pda_payer_pda = pda_payer_pda().0;
    let pda_payer_balance = ctx.balance(&pda_payer_pda);

    let sender = ctx.sender.pubkey();
    let handle_account_metas = ctx.hyper_prover().handle_account_metas(
        intent.destination.try_into().unwrap(),
        sender.to_bytes(),
        payload,
    );
    ctx.hyperlane()
        .inbox_process(message, handle_account_metas)
        .unwrap();

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        intent.route.hash(),
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda, false)),
    );
    assert!(
        result.is_ok_and(common::contains_event(IntentWithdrawn::new(
            intent_hash,
            claimant,
        )))
    );
    let proof = ctx.get_account(&proof).unwrap();
    assert!(proof.data.is_empty());
    assert_eq!(proof.owner, system_program::ID);
    assert_eq!(proof.lamports, 0);
    assert_eq!(pda_payer_balance, ctx.balance(&pda_payer_pda));
}

#[test]
fn handle_invalid_config_fail() {
    let mut ctx = setup();
    let destination = random();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let intent_hash: Bytes32 = random::<[u8; 32]>().into();
    let payload = claimant
        .iter()
        .copied()
        .chain(intent_hash.to_vec())
        .collect::<Vec<_>>();
    let message = create_hyperlane_message(
        ctx.sender.pubkey().to_bytes().into(),
        destination,
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );

    let sender = ctx.sender.pubkey();
    let mut handle_account_metas =
        ctx.hyper_prover()
            .handle_account_metas(destination, sender.to_bytes(), payload);
    *handle_account_metas.get_mut(0).unwrap() =
        AccountMeta::new_readonly(Pubkey::new_unique(), false);
    let result = ctx.hyperlane().inbox_process(message, handle_account_metas);
    assert!(result.is_err_and(common::is_error(ErrorCode::AccountNotInitialized)))
}

#[test]
fn handle_invalid_sender_fail() {
    let mut ctx = setup();
    let destination = random();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let intent_hash: Bytes32 = random::<[u8; 32]>().into();
    let payload = claimant
        .iter()
        .copied()
        .chain(intent_hash.to_vec())
        .collect::<Vec<_>>();
    let message = create_hyperlane_message(
        Pubkey::new_unique().to_bytes().into(),
        destination,
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );

    let sender = ctx.sender.pubkey();
    let handle_account_metas =
        ctx.hyper_prover()
            .handle_account_metas(destination, sender.to_bytes(), payload);
    let result = ctx.hyperlane().inbox_process(message, handle_account_metas);
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidSender)))
}

#[test]
fn handle_invalid_proof_fail() {
    let mut ctx = setup();
    let destination = random();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let intent_hash: Bytes32 = random::<[u8; 32]>().into();
    let payload = claimant
        .iter()
        .copied()
        .chain(intent_hash.to_vec())
        .collect::<Vec<_>>();
    let message = create_hyperlane_message(
        ctx.sender.pubkey().to_bytes().into(),
        destination,
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );

    let sender = ctx.sender.pubkey();
    let mut handle_account_metas =
        ctx.hyper_prover()
            .handle_account_metas(destination, sender.to_bytes(), payload);
    *handle_account_metas.get_mut(1).unwrap() = AccountMeta::new(Pubkey::new_unique(), false);
    let result = ctx.hyperlane().inbox_process(message, handle_account_metas);
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidProof)))
}

#[test]
fn handle_invalid_pda_payer_fail() {
    let mut ctx = setup();
    let destination = random();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let intent_hash: Bytes32 = random::<[u8; 32]>().into();
    let payload = claimant
        .iter()
        .copied()
        .chain(intent_hash.to_vec())
        .collect::<Vec<_>>();
    let message = create_hyperlane_message(
        ctx.sender.pubkey().to_bytes().into(),
        destination,
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );

    let sender = ctx.sender.pubkey();
    let mut handle_account_metas =
        ctx.hyper_prover()
            .handle_account_metas(destination, sender.to_bytes(), payload);
    *handle_account_metas.get_mut(3).unwrap() = AccountMeta::new(Pubkey::new_unique(), false);
    let result = ctx.hyperlane().inbox_process(message, handle_account_metas);
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidPdaPayer)))
}

#[test]
fn handle_already_proven_fail() {
    let mut ctx = setup();
    let destination = random();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let intent_hash: Bytes32 = random::<[u8; 32]>().into();
    let payload = claimant
        .iter()
        .copied()
        .chain(intent_hash.to_vec())
        .collect::<Vec<_>>();
    let message = create_hyperlane_message(
        ctx.sender.pubkey().to_bytes().into(),
        destination,
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );

    let sender = ctx.sender.pubkey();
    let handle_account_metas =
        ctx.hyper_prover()
            .handle_account_metas(destination, sender.to_bytes(), payload.clone());
    ctx.hyperlane()
        .inbox_process(message, handle_account_metas)
        .unwrap();

    let message = create_hyperlane_message(
        ctx.sender.pubkey().to_bytes().into(),
        destination,
        CHAIN_ID.try_into().unwrap(),
        hyper_prover::ID.to_bytes().into(),
        payload.clone(),
    );
    let handle_account_metas =
        ctx.hyper_prover()
            .handle_account_metas(destination, sender.to_bytes(), payload);
    let result = ctx.hyperlane().inbox_process(message, handle_account_metas);
    assert!(result.is_err_and(common::is_error(HyperProverError::IntentAlreadyProven)))
}
