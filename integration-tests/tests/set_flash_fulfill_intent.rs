use anchor_lang::error::ErrorCode;
use anchor_lang::solana_program::system_instruction;
use eco_svm_std::CHAIN_ID;
use flash_fulfiller::instructions::FlashFulfillerError;
use flash_fulfiller::state::FlashFulfillIntentAccount;
use portal::types::intent_hash;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

pub mod common;

#[test]
fn set_flash_fulfill_intent_should_succeed() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

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
fn set_flash_fulfill_intent_handles_pre_funded_pda() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    let griefer = Keypair::new();
    ctx.airdrop(&griefer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let pre_funding = 1_000u64;
    let prefund_tx = Transaction::new(
        &[&griefer],
        Message::new(
            &[system_instruction::transfer(
                &griefer.pubkey(),
                &buffer,
                pre_funding,
            )],
            Some(&griefer.pubkey()),
        ),
        ctx.latest_blockhash(),
    );
    ctx.send_transaction(prefund_tx).unwrap();
    assert_eq!(ctx.balance(&buffer), pre_funding);

    let writer_balance_before = ctx.balance(&writer.pubkey());

    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route.clone(), reward.clone())
        .unwrap();

    let raw = ctx.get_account(&buffer).unwrap();
    assert_eq!(raw.owner, flash_fulfiller::ID);

    let stored = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert_eq!(stored.writer, writer.pubkey());
    assert_eq!(stored.route.hash(), route.hash());
    assert_eq!(stored.reward.hash(), reward.hash());

    let buffer_rent = ctx.balance(&buffer);
    assert_eq!(
        ctx.balance(&writer.pubkey()),
        writer_balance_before - (buffer_rent - pre_funding),
    );
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
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route.clone(), reward.clone())
        .unwrap();

    let result = ctx
        .flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route, reward);

    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintZero)));
}

#[test]
fn set_flash_fulfill_intent_pda_isolated_per_writer() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

    let writer_a = ctx.payer.insecure_clone();
    let buffer_a = FlashFulfillIntentAccount::pda(&writer_a.pubkey(), &intent_hash_value).0;

    let writer_b = Keypair::new();
    ctx.airdrop(&writer_b.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let buffer_b = FlashFulfillIntentAccount::pda(&writer_b.pubkey(), &intent_hash_value).0;

    assert_ne!(buffer_a, buffer_b);

    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer_a, buffer_a, route.clone(), reward.clone())
        .unwrap();
    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer_b, buffer_b, route, reward)
        .unwrap();

    let stored_a = ctx.account::<FlashFulfillIntentAccount>(&buffer_a).unwrap();
    let stored_b = ctx.account::<FlashFulfillIntentAccount>(&buffer_b).unwrap();
    assert_eq!(stored_a.writer, writer_a.pubkey());
    assert_eq!(stored_b.writer, writer_b.pubkey());
}
