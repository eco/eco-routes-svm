use anchor_lang::{AccountDeserialize, AnchorSerialize, Discriminator};
use eco_svm_std::CHAIN_ID;
use flash_fulfiller::instructions::FlashFulfillerError;
use flash_fulfiller::state::FlashFulfillIntentAccount;
use portal::types::intent_hash;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn close_flash_fulfill_intent_should_succeed() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route, reward)
        .unwrap();

    let writer_balance_before = ctx.balance(&writer.pubkey());
    let buffer_rent = ctx.balance(&buffer);

    let result = ctx
        .flash_fulfiller()
        .close_flash_fulfill_intent(&writer, intent_hash_value);
    assert!(result.is_ok());

    assert!(ctx.account::<FlashFulfillIntentAccount>(&buffer).is_none());
    assert_eq!(
        ctx.balance(&writer.pubkey()),
        writer_balance_before + buffer_rent,
    );
}

#[test]
fn close_flash_fulfill_intent_non_writer_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route, reward)
        .unwrap();

    let mallory = Keypair::new();
    ctx.airdrop(&mallory.pubkey(), common::sol_amount(1.0))
        .unwrap();

    let result = ctx
        .flash_fulfiller()
        .close_flash_fulfill_intent(&mallory, intent_hash_value);

    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::InvalidFlashFulfillIntentAccount
    )));
    assert!(ctx.account::<FlashFulfillIntentAccount>(&buffer).is_some());
}

#[test]
fn close_flash_fulfill_intent_reclaims_malformed_buffer() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    let mut malformed = Vec::new();
    malformed.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    malformed.extend_from_slice(&[0xAA; 7]);

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, malformed.clone())
        .unwrap();

    assert!(ctx.account::<FlashFulfillIntentAccount>(&buffer).is_none());
    let raw_before = ctx.get_account(&buffer).unwrap();
    assert_eq!(raw_before.owner, flash_fulfiller::ID);
    assert!(FlashFulfillIntentAccount::try_deserialize(&mut raw_before.data.as_slice()).is_err());

    let writer_balance_before = ctx.balance(&writer.pubkey());
    let buffer_rent = ctx.balance(&buffer);

    let result = ctx
        .flash_fulfiller()
        .close_flash_fulfill_intent(&writer, intent_hash_value);
    assert!(result.is_ok());

    assert_eq!(ctx.balance(&buffer), 0);
    assert_eq!(
        ctx.balance(&writer.pubkey()),
        writer_balance_before + buffer_rent,
    );
}

#[test]
fn close_flash_fulfill_intent_closes_append_built_buffer() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    let mut payload = Vec::new();
    payload.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    payload.extend_from_slice(writer.pubkey().as_ref());
    payload.extend_from_slice(&route.try_to_vec().unwrap());
    payload.extend_from_slice(&reward.try_to_vec().unwrap());

    let split = payload.len() / 2;
    let first_chunk = payload[..split].to_vec();
    let second_chunk = payload[split..].to_vec();

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, first_chunk)
        .unwrap();
    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, second_chunk)
        .unwrap();

    let stored = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert_eq!(stored.writer, writer.pubkey());
    assert_eq!(stored.route.hash(), route.hash());
    assert_eq!(stored.reward.hash(), reward.hash());

    let writer_balance_before = ctx.balance(&writer.pubkey());
    let buffer_rent = ctx.balance(&buffer);

    let result = ctx
        .flash_fulfiller()
        .close_flash_fulfill_intent(&writer, intent_hash_value);
    assert!(result.is_ok());

    assert_eq!(ctx.balance(&buffer), 0);
    assert!(ctx.account::<FlashFulfillIntentAccount>(&buffer).is_none());
    assert_eq!(
        ctx.balance(&writer.pubkey()),
        writer_balance_before + buffer_rent,
    );
}
