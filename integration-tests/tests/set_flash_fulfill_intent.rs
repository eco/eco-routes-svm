use anchor_lang::error::ErrorCode;
use eco_svm_std::CHAIN_ID;
use flash_fulfiller::instructions::FlashFulfillerError;
use flash_fulfiller::state::FlashFulfillIntentAccount;
use portal::types::intent_hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn set_flash_fulfill_intent_should_succeed() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value).0;
    let writer = ctx.payer.insecure_clone();

    let result = ctx.flash_fulfiller().set_flash_fulfill_intent(
        &writer,
        buffer,
        route.clone(),
        reward.clone(),
    );
    assert!(result.is_ok());

    let stored = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert_eq!(stored.writer, writer.pubkey());
    assert_eq!(stored.route.hash(), route.hash());
    assert_eq!(stored.reward.hash(), reward.hash());
}

#[test]
fn set_flash_fulfill_intent_wrong_pda_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let writer = ctx.payer.insecure_clone();
    let result = ctx.flash_fulfiller().set_flash_fulfill_intent(
        &writer,
        Pubkey::new_unique(),
        route,
        reward,
    );

    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::InvalidFlashFulfillIntentAccount,
    )));
}

#[test]
fn set_flash_fulfill_intent_already_exists_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value).0;
    let writer = ctx.payer.insecure_clone();

    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route.clone(), reward.clone())
        .unwrap();

    let result = ctx
        .flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route, reward);

    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintZero)));
}
