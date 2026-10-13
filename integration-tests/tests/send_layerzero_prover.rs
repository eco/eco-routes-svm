use anchor_lang::error::ErrorCode;
use eco_svm_std::Bytes32;
use layerzero_prover::constants::lz_receive_gas;
use layerzero_prover::instructions::{LayerZeroProverError, QuoteMessageArgs};
use layerzero_prover::layerzero::{lz_receive_options, MessagingFee};
use layerzero_prover::state::{PendingSend, Store};
use mock_layerzero_endpoint::{MockEndpointError, MockPacketSent, MOCK_NATIVE_FEE};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{peers, BASE_EID, TREASURY};

pub mod common;

/// Two fulfilled intents proven to Base; returns (context, pending, payload, fee payer).
fn committed() -> (common::Context, Pubkey, Vec<u8>, Keypair) {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let hashes: Vec<Bytes32> = context
        .fulfill_rand_intents(2, layerzero_prover::ID)
        .iter()
        .map(|intent| intent.intent_hash)
        .collect();
    let receiver = peers()[0].address;
    context
        .layerzero_prover()
        .prove(hashes.clone(), BASE_EID.into(), receiver.to_vec())
        .unwrap();
    let payload = context.layerzero_prover().payload(&hashes);
    let pending = PendingSend::pda(BASE_EID, &receiver, &payload).0;
    let fee_payer = Keypair::new();
    context.airdrop(&fee_payer.pubkey(), 1_000_000_000).unwrap();

    (context, pending, payload, fee_payer)
}

#[test]
fn send_message_dispatches_with_floor_options_and_refunds_commit_rent() {
    let (mut context, pending, payload, fee_payer) = committed();
    let rent_payer = context.payer.pubkey();
    let receiver = peers()[0].address;
    let commit_rent = context.get_account(&pending).unwrap().lamports;
    let rent_payer_before = context.balance(&rent_payer);

    let result = context
        .layerzero_prover()
        .send_message(
            pending,
            rent_payer,
            &fee_payer,
            BASE_EID,
            &receiver,
            MOCK_NATIVE_FEE,
        )
        .unwrap();

    assert!(common::contains_event(MockPacketSent {
        sender: Store::pda().0,
        dst_eid: BASE_EID,
        receiver: receiver.into(),
        message: payload,
        options: lz_receive_options(lz_receive_gas(2)),
        native_fee: MOCK_NATIVE_FEE,
        nonce: 1,
    })(result));
    assert!(context.get_account(&pending).is_none());
    assert_eq!(
        context.balance(&rent_payer),
        rent_payer_before + commit_rent
    );
    assert_eq!(context.balance(&TREASURY), MOCK_NATIVE_FEE);
}

#[test]
fn second_send_fails_once_commit_closed() {
    let (mut context, pending, _, fee_payer) = committed();
    let rent_payer = context.payer.pubkey();
    let receiver = peers()[0].address;
    context
        .layerzero_prover()
        .send_message(
            pending,
            rent_payer,
            &fee_payer,
            BASE_EID,
            &receiver,
            MOCK_NATIVE_FEE,
        )
        .unwrap();
    context.expire_blockhash();

    let result = context.layerzero_prover().send_message(
        pending,
        rent_payer,
        &fee_payer,
        BASE_EID,
        &receiver,
        MOCK_NATIVE_FEE,
    );

    assert!(result.is_err_and(common::is_error(ErrorCode::AccountNotInitialized)));
    assert_eq!(context.balance(&TREASURY), MOCK_NATIVE_FEE);
}

#[test]
fn send_message_rejects_foreign_rent_payer() {
    let (mut context, pending, _, fee_payer) = committed();
    let receiver = peers()[0].address;

    let result = context.layerzero_prover().send_message(
        pending,
        fee_payer.pubkey(),
        &fee_payer,
        BASE_EID,
        &receiver,
        MOCK_NATIVE_FEE,
    );

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidRentPayer)));
}

#[test]
fn short_max_fee_fails_and_keeps_commit() {
    let (mut context, pending, _, fee_payer) = committed();
    let rent_payer = context.payer.pubkey();
    let receiver = peers()[0].address;

    let result = context.layerzero_prover().send_message(
        pending,
        rent_payer,
        &fee_payer,
        BASE_EID,
        &receiver,
        MOCK_NATIVE_FEE - 1,
    );

    assert!(result.is_err_and(common::is_error(MockEndpointError::InsufficientFee)));
    assert!(context.get_account(&pending).is_some());
}

#[test]
fn quote_message_returns_endpoint_fee() {
    let (mut context, _, payload, _) = committed();

    let fee = context
        .layerzero_prover()
        .quote_message(QuoteMessageArgs {
            dst_eid: BASE_EID,
            receiver: peers()[0].address,
            payload,
        })
        .unwrap();

    assert_eq!(
        fee,
        MessagingFee {
            native_fee: MOCK_NATIVE_FEE,
            lz_token_fee: 0
        }
    );
}

#[test]
fn quote_message_rejects_unknown_peer_and_wrong_receiver() {
    let (mut context, _, payload, _) = committed();

    let unknown = context.layerzero_prover().quote_message(QuoteMessageArgs {
        dst_eid: 40_245,
        receiver: peers()[0].address,
        payload: payload.clone(),
    });
    assert!(unknown.is_err_and(common::is_error(LayerZeroProverError::UnknownPeer)));

    let wrong = context.layerzero_prover().quote_message(QuoteMessageArgs {
        dst_eid: BASE_EID,
        receiver: peers()[1].address,
        payload,
    });
    assert!(wrong.is_err_and(common::is_error(LayerZeroProverError::InvalidReceiver)));
}
