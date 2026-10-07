use std::iter;

use anchor_lang::AnchorDeserialize;
use eco_svm_std::prover::{IntentHashClaimant, IntentProven, Proof, ProofData};
use eco_svm_std::{Bytes32, CANCELLED};
use layerzero_prover::instructions::LayerZeroProverError;
use layerzero_prover::layerzero::{self, LzInstruction, LzReceiveTypesV2Result};
use layerzero_prover::state::{pda_payer_pda, ProofAccount, Store};
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use solana_sdk::account::Account;
use solana_sdk::instruction::AccountMeta;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{
    evm_peer, peers, receive_params, BASE_CHAIN_ID, BASE_EID, OP_CHAIN_ID,
};

pub mod common;

fn ready() -> common::Context {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    context
}

fn pairs(hashes: &[(u8, Pubkey)]) -> ProofData {
    ProofData::new(
        BASE_CHAIN_ID,
        hashes
            .iter()
            .map(|(byte, claimant)| {
                IntentHashClaimant::new([*byte; 32].into(), claimant.to_bytes().into())
            })
            .collect(),
    )
}

fn proof(context: &common::Context, byte: u8) -> Option<Proof> {
    context
        .account::<ProofAccount>(&Proof::pda(&[byte; 32].into(), &layerzero_prover::ID).0)
        .map(|account| account.0)
}

#[test]
fn delivers_and_creates_proofs() {
    let mut context = ready();
    let (alice, bob) = (Pubkey::new_unique(), Pubkey::new_unique());
    let params = receive_params(&peers()[0], 1, pairs(&[(1, alice), (2, bob)]));
    let payload_hash =
        layerzero::payload_hash_pda(&Store::pda().0, params.src_eid, &params.sender, 1).0;

    let result = context.layerzero_prover().deliver(&params).unwrap();

    assert_eq!(proof(&context, 1).unwrap().claimant, alice);
    assert_eq!(proof(&context, 2).unwrap().destination, BASE_CHAIN_ID);
    assert!(common::contains_cpi_event(IntentProven::new(
        [1; 32].into(),
        alice,
        BASE_CHAIN_ID
    ))(result));
    assert!(context.get_account(&payload_hash).is_none());
}

/// The executor builds `lz_receive` from `lz_receive_types_v2`'s answer.
#[test]
fn delivery_built_from_v2_discovery_succeeds() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().verify(&params).unwrap();
    let discovered = context
        .layerzero_prover()
        .lz_receive_types_v2(params.clone())
        .unwrap();
    let mut decoded = LzReceiveTypesV2Result::try_from_slice(&discovered.return_data.data).unwrap();
    let LzInstruction::LzReceive { accounts } = decoded.instructions.remove(0) else {
        panic!("expected an LzReceive instruction")
    };

    let instruction = context
        .layerzero_prover()
        .lz_receive_instruction(&params, accounts);
    assert!(context
        .layerzero_prover()
        .send(vec![instruction], &[])
        .is_ok());
    assert!(proof(&context, 1).is_some());
}

#[test]
fn replayed_delivery_fails() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().deliver(&params).unwrap();
    context.expire_blockhash();

    assert!(context.layerzero_prover().lz_receive(&params).is_err());
}

#[test]
fn chain_id_mismatch_rejected() {
    let mut context = ready();
    let mut data = pairs(&[(1, Pubkey::new_unique())]);
    data.destination = OP_CHAIN_ID;
    let params = receive_params(&peers()[0], 1, data);

    let result = context.layerzero_prover().deliver(&params);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::ChainIdMismatch)));
    assert!(proof(&context, 1).is_none());
}

#[test]
fn non_peer_sender_rejected_even_if_endpoint_path_exists() {
    let mut context = ready();
    let stranger = evm_peer(BASE_EID, BASE_CHAIN_ID, 0x66);
    context
        .layerzero_prover()
        .force_nonce(stranger.eid, stranger.address.into());
    let params = receive_params(&stranger, 1, pairs(&[(1, Pubkey::new_unique())]));

    let result = context.layerzero_prover().deliver(&params);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidSender)));
}

#[test]
fn wrong_receiver_in_clear_accounts_rejected() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().verify(&params).unwrap();
    let mut accounts = layerzero_prover::instructions::lz_receive_accounts(
        &params,
        &ProofData::from_bytes(&params.message).unwrap(),
    );
    accounts[6].pubkey = layerzero::AddressLocator::Address(Pubkey::new_unique());

    let instruction = context
        .layerzero_prover()
        .lz_receive_instruction(&params, accounts);
    let result = context.layerzero_prover().send(vec![instruction], &[]);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidStore)));
}

#[test]
fn proof_account_mismatch_rejected() {
    let mut context = ready();
    let params = receive_params(
        &peers()[0],
        1,
        pairs(&[(1, Pubkey::new_unique()), (2, Pubkey::new_unique())]),
    );
    context.layerzero_prover().verify(&params).unwrap();
    let mut accounts = layerzero_prover::instructions::lz_receive_accounts(
        &params,
        &ProofData::from_bytes(&params.message).unwrap(),
    );
    accounts.pop();

    let instruction = context
        .layerzero_prover()
        .lz_receive_instruction(&params, accounts);
    let result = context.layerzero_prover().send(vec![instruction], &[]);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidProof)));
}

#[test]
fn redelivery_is_idempotent_and_conflict_fails() {
    let mut context = ready();
    let alice = Pubkey::new_unique();
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, pairs(&[(1, alice)])))
        .unwrap();

    let same =
        context
            .layerzero_prover()
            .deliver(&receive_params(&peers()[0], 2, pairs(&[(1, alice)])));
    assert!(same.is_ok());

    let conflict = context.layerzero_prover().deliver(&receive_params(
        &peers()[0],
        3,
        pairs(&[(1, Pubkey::new_unique())]),
    ));
    assert!(conflict.is_err_and(common::is_error(LayerZeroProverError::IntentAlreadyProven)));
    assert_eq!(proof(&context, 1).unwrap().claimant, alice);
}

#[test]
fn duplicate_pair_in_one_message_is_idempotent_and_conflict_fails() {
    let mut context = ready();
    let alice = Pubkey::new_unique();

    let same = context.layerzero_prover().deliver(&receive_params(
        &peers()[0],
        1,
        pairs(&[(1, alice), (1, alice)]),
    ));
    assert!(same.is_ok());

    let conflict = context.layerzero_prover().deliver(&receive_params(
        &peers()[0],
        2,
        pairs(&[(2, alice), (2, Pubkey::new_unique())]),
    ));
    assert!(conflict.is_err_and(common::is_error(LayerZeroProverError::IntentAlreadyProven)));
    assert!(proof(&context, 2).is_none());
}

#[test]
fn underfunded_pda_payer_fails_then_retry_succeeds() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    let payload_hash =
        layerzero::payload_hash_pda(&Store::pda().0, params.src_eid, &params.sender, 1).0;
    context.layerzero_prover().verify(&params).unwrap();
    context
        .set_account(
            pda_payer_pda().0,
            Account {
                lamports: 0,
                data: vec![],
                owner: anchor_lang::system_program::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();

    assert!(context.layerzero_prover().lz_receive(&params).is_err());
    assert!(context.get_account(&payload_hash).is_some());

    context.airdrop(&pda_payer_pda().0, 1_000_000_000).unwrap();
    assert!(context.layerzero_prover().lz_receive(&params).is_ok());
    assert!(proof(&context, 1).is_some());
}

#[test]
fn cancelled_claimant_passes_through() {
    let mut context = ready();
    let cancelled = Pubkey::new_from_array(CANCELLED.into());

    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, pairs(&[(1, cancelled)])))
        .unwrap();

    assert_eq!(proof(&context, 1).unwrap().claimant, cancelled);
}

#[test]
fn delivered_proof_withdraws_and_refunds_rent_to_pda_payer() {
    let mut context = ready();
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let claimant = Pubkey::new_unique();
    let data = ProofData::new(
        BASE_CHAIN_ID,
        vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
    );
    let pda_payer_before = context.balance(&pda_payer_pda().0);
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, data))
        .unwrap();
    let proof_address = Proof::pda(&hash, &layerzero_prover::ID).0;

    let result = context.portal().withdraw_intent(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        claimant,
        proof_address,
        WithdrawnMarker::pda(&hash).0,
        proof_closer_pda(&layerzero_prover::ID).0,
        Vec::<AccountMeta>::new(),
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );

    assert!(result.is_ok());
    assert_eq!(context.balance(&claimant), reward.native_amount);
    assert!(context.get_account(&proof_address).is_none());
    assert_eq!(context.balance(&pda_payer_pda().0), pda_payer_before);
}

#[test]
fn delivered_proof_aggregates() {
    let mut context = ready();
    let aggregator_authority = Keypair::new();
    context
        .aggregator_prover()
        .install(aggregator_authority.pubkey());
    context
        .aggregator_prover()
        .init(&aggregator_authority, vec![layerzero_prover::ID])
        .unwrap();
    let claimant = Pubkey::new_unique();
    let hash: Bytes32 = [9; 32].into();
    let data = ProofData::new(
        BASE_CHAIN_ID,
        vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
    );
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, data))
        .unwrap();

    let result = context
        .aggregator_prover()
        .aggregate(hash, layerzero_prover::ID);

    assert!(result.is_ok());
    assert!(context
        .get_account(&Proof::pda(&hash, &aggregator_prover::ID).0)
        .is_some());
}

#[test]
fn proven_cancellation_refunds_through_close_proof() {
    let mut context = ready();
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let data = ProofData::new(
        BASE_CHAIN_ID,
        vec![IntentHashClaimant::new(hash, CANCELLED)],
    );
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, data))
        .unwrap();
    let proof_address = Proof::pda(&hash, &layerzero_prover::ID).0;

    let result = context.portal().refund_intent_with_close_proof(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        proof_address,
        WithdrawnMarker::pda(&hash).0,
        reward.creator,
        Vec::<AccountMeta>::new(),
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_ok());
    assert_eq!(context.balance(&reward.creator), reward.native_amount);
    assert!(context.get_account(&proof_address).is_none());
}
