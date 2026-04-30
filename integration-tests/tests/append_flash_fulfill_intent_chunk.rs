use anchor_lang::solana_program::system_instruction;
use anchor_lang::{AnchorSerialize, Discriminator};
use eco_svm_std::CHAIN_ID;
use flash_fulfiller::state::FlashFulfillIntentAccount;
use portal::types::intent_hash;
use solana_sdk::message::Message;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

pub mod common;

#[test]
fn append_flash_fulfill_intent_chunk_first_call_creates_buffer() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    let mut payload = Vec::new();
    payload.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    payload.extend_from_slice(writer.pubkey().as_ref());
    payload.extend_from_slice(&route.try_to_vec().unwrap());
    payload.extend_from_slice(&reward.try_to_vec().unwrap());

    let result = ctx.flash_fulfiller().append_flash_fulfill_intent_chunk(
        &writer,
        intent_hash_value,
        payload.clone(),
    );
    assert!(result.is_ok());

    let raw = ctx.get_account(&buffer).unwrap();
    assert_eq!(raw.data, payload);
    assert_eq!(raw.owner, flash_fulfiller::ID);

    let stored = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert_eq!(stored.writer, writer.pubkey());
    assert_eq!(stored.route.hash(), route.hash());
    assert_eq!(stored.reward.hash(), reward.hash());
}

#[test]
fn append_flash_fulfill_intent_chunk_extends_buffer() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
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
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, first_chunk.clone())
        .unwrap();
    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, second_chunk.clone())
        .unwrap();

    let raw = ctx.get_account(&buffer).unwrap();
    assert_eq!(raw.data.len(), first_chunk.len() + second_chunk.len());
    assert_eq!(raw.data, payload);

    let stored = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert_eq!(stored.writer, writer.pubkey());
    assert_eq!(stored.route.hash(), route.hash());
    assert_eq!(stored.reward.hash(), reward.hash());
}

#[test]
fn append_flash_fulfill_intent_chunk_non_writer_isolated() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
    let writer_buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    let mut payload = Vec::new();
    payload.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    payload.extend_from_slice(writer.pubkey().as_ref());
    payload.extend_from_slice(&route.try_to_vec().unwrap());
    payload.extend_from_slice(&reward.try_to_vec().unwrap());

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, payload.clone())
        .unwrap();

    let mallory = Keypair::new();
    ctx.airdrop(&mallory.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let mallory_buffer = FlashFulfillIntentAccount::pda(&mallory.pubkey(), &intent_hash_value).0;

    assert_ne!(writer_buffer, mallory_buffer);

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&mallory, intent_hash_value, vec![0xAA; 16])
        .unwrap();

    assert_eq!(ctx.get_account(&writer_buffer).unwrap().data, payload);
    assert_eq!(
        ctx.get_account(&mallory_buffer).unwrap().data,
        vec![0xAA; 16],
    );
}

#[test]
fn append_flash_fulfill_intent_chunk_isolated_per_writer() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let alice = ctx.payer.insecure_clone();
    let alice_buffer = FlashFulfillIntentAccount::pda(&alice.pubkey(), &intent_hash_value).0;

    let mut payload = Vec::new();
    payload.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    payload.extend_from_slice(alice.pubkey().as_ref());
    payload.extend_from_slice(&route.try_to_vec().unwrap());
    payload.extend_from_slice(&reward.try_to_vec().unwrap());

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&alice, intent_hash_value, payload.clone())
        .unwrap();

    let alice_data_before = ctx.get_account(&alice_buffer).unwrap().data;
    let alice_lamports_before = ctx.balance(&alice_buffer);

    let mallory = Keypair::new();
    ctx.airdrop(&mallory.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let mallory_buffer = FlashFulfillIntentAccount::pda(&mallory.pubkey(), &intent_hash_value).0;

    assert_ne!(alice_buffer, mallory_buffer);

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&mallory, intent_hash_value, vec![0xCC; 32])
        .unwrap();

    assert_eq!(ctx.get_account(&alice_buffer).unwrap().data, payload);
    assert_eq!(
        ctx.get_account(&alice_buffer).unwrap().data,
        alice_data_before
    );
    assert_eq!(ctx.balance(&alice_buffer), alice_lamports_before);
    assert_eq!(
        ctx.get_account(&mallory_buffer).unwrap().data,
        vec![0xCC; 32],
    );
}

#[test]
fn append_flash_fulfill_intent_chunk_handles_pre_funded_pda() {
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

    let mut payload = Vec::new();
    payload.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    payload.extend_from_slice(writer.pubkey().as_ref());
    payload.extend_from_slice(&route.try_to_vec().unwrap());
    payload.extend_from_slice(&reward.try_to_vec().unwrap());

    let writer_balance_before = ctx.balance(&writer.pubkey());

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, payload.clone())
        .unwrap();

    let raw = ctx.get_account(&buffer).unwrap();
    assert_eq!(raw.data, payload);
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
fn append_flash_fulfill_intent_chunk_tops_up_rent_on_growth() {
    let mut ctx = common::Context::default();
    let intent_hash_value = [0xABu8; 32].into();
    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    let first_chunk = vec![0x11u8; 50];
    let second_chunk = vec![0x22u8; 500];

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, first_chunk.clone())
        .unwrap();

    let rent = ctx.get_sysvar::<Rent>();
    let buffer_lamports_after_first = ctx.balance(&buffer);
    let writer_balance_after_first = ctx.balance(&writer.pubkey());
    assert_eq!(
        buffer_lamports_after_first,
        rent.minimum_balance(first_chunk.len()),
    );

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, second_chunk.clone())
        .unwrap();

    let raw = ctx.get_account(&buffer).unwrap();
    assert_eq!(raw.data.len(), first_chunk.len() + second_chunk.len());
    assert_eq!(&raw.data[..first_chunk.len()], &first_chunk[..]);
    assert_eq!(&raw.data[first_chunk.len()..], &second_chunk[..]);
    assert_eq!(raw.owner, flash_fulfiller::ID);

    let buffer_lamports_after_second = ctx.balance(&buffer);
    let expected_top_up = buffer_lamports_after_second - buffer_lamports_after_first;
    assert!(expected_top_up > 0);
    assert_eq!(
        ctx.balance(&writer.pubkey()),
        writer_balance_after_first - expected_top_up,
    );
}
