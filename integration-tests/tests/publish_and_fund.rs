use anchor_lang::AnchorSerialize;
use compact_portal::instructions::route_to_abi;
use eco_svm_std::CHAIN_ID;
use portal::events::{IntentFunded, IntentPublished};
use portal::instructions::PortalError;
use portal::state;
use portal::types::intent_hash;
use solana_sdk::signer::Signer;
use tiny_keccak::{Hasher, Keccak};

pub mod common;

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];
    hasher.update(data);
    hasher.finalize(&mut hash);
    hash
}

fn encode_route(destination: u64, route: &portal::types::Route) -> Vec<u8> {
    if destination == CHAIN_ID {
        route.try_to_vec().unwrap()
    } else {
        route_to_abi(route.clone())
    }
}

#[test]
fn publish_and_fund_native_success() {
    let mut ctx = common::Context::default();
    let (destination, route, reward) = ctx.rand_intent();
    let route_bytes: Vec<u8> = encode_route(destination, &route);
    let route_hash = keccak256(&route_bytes);
    let intent_h = intent_hash(destination, &route_hash.into(), &reward.hash());
    let vault_pda = state::vault_pda(&intent_h).0;
    let fund_amount = reward.native_amount;
    let funder = ctx.funder.pubkey();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());

    ctx.airdrop(&funder, fund_amount).unwrap();

    let result = ctx.compact_portal().publish_and_fund(
        destination,
        route,
        reward.clone(),
        vault_pda,
        true,
        vec![],
    );

    assert!(result
        .clone()
        .is_ok_and(common::contains_event(IntentPublished::new(
            intent_h,
            destination,
            route_bytes,
            reward.clone(),
        ))));
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_h,
        ctx.funder.pubkey(),
        false,
    ))));
    assert_eq!(ctx.balance(&vault_pda), fund_amount);
    assert_eq!(ctx.balance(&funder), 0);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
}

#[test]
fn publish_and_fund_native_partial_success() {
    let mut ctx = common::Context::default();
    let (destination, route, reward) = ctx.rand_intent();
    let route_bytes: Vec<u8> = encode_route(destination, &route);
    let route_hash = keccak256(&route_bytes);
    let intent_h = intent_hash(destination, &route_hash.into(), &reward.hash());
    let vault_pda = state::vault_pda(&intent_h).0;
    let partial_amount = reward.native_amount / 2;
    let funder = ctx.funder.pubkey();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());

    ctx.airdrop(&funder, partial_amount).unwrap();

    let result = ctx.compact_portal().publish_and_fund(
        destination,
        route,
        reward.clone(),
        vault_pda,
        true,
        vec![],
    );

    assert!(result
        .clone()
        .is_ok_and(common::contains_event(IntentPublished::new(
            intent_h,
            destination,
            route_bytes,
            reward.clone(),
        ))));
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_h,
        ctx.funder.pubkey(),
        false,
    ))));
    assert_eq!(ctx.balance(&vault_pda), partial_amount);
    assert_eq!(ctx.balance(&funder), 0);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
}

#[test]
fn publish_and_fund_native_only_fully_funded_success() {
    let mut ctx = common::Context::default();
    let (destination, route, mut reward) = ctx.rand_intent();
    reward.tokens.clear();
    let route_bytes: Vec<u8> = encode_route(destination, &route);
    let route_hash = keccak256(&route_bytes);
    let intent_h = intent_hash(destination, &route_hash.into(), &reward.hash());
    let vault_pda = state::vault_pda(&intent_h).0;
    let fund_amount = reward.native_amount;
    let funder = ctx.funder.pubkey();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());

    ctx.airdrop(&funder, fund_amount).unwrap();

    let result = ctx.compact_portal().publish_and_fund(
        destination,
        route,
        reward.clone(),
        vault_pda,
        false,
        vec![],
    );

    assert!(result
        .clone()
        .is_ok_and(common::contains_event(IntentPublished::new(
            intent_h,
            destination,
            route_bytes,
            reward.clone(),
        ))));
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_h,
        ctx.funder.pubkey(),
        true,
    ))));
    assert_eq!(ctx.balance(&vault_pda), fund_amount);
    assert_eq!(ctx.balance(&funder), 0);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
}

#[test]
fn publish_and_fund_invalid_vault_fails() {
    let mut ctx = common::Context::default();
    let (destination, route, reward) = ctx.rand_intent();
    let fund_amount = reward.native_amount;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, fund_amount).unwrap();

    let result = ctx.compact_portal().publish_and_fund(
        destination,
        route,
        reward,
        solana_sdk::pubkey::Pubkey::new_unique(),
        true,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(PortalError::InvalidVault)));
}

#[test]
fn publish_and_fund_insufficient_native_funds_fails() {
    let mut ctx = common::Context::default();
    let (destination, route, reward) = ctx.rand_intent();
    let route_bytes: Vec<u8> = encode_route(destination, &route);
    let route_hash = keccak256(&route_bytes);
    let intent_h = intent_hash(destination, &route_hash.into(), &reward.hash());
    let vault_pda = state::vault_pda(&intent_h).0;
    let funder = ctx.funder.pubkey();

    let insufficient_amount = reward.native_amount / 2;
    ctx.airdrop(&funder, insufficient_amount).unwrap();

    let result =
        ctx.compact_portal()
            .publish_and_fund(destination, route, reward, vault_pda, false, vec![]);
    assert!(result.is_err_and(common::is_error(PortalError::InsufficientFunds)));
}

#[test]
fn publish_and_fund_solana_destination_uses_borsh() {
    let mut ctx = common::Context::default();
    let (_, route, reward) = ctx.rand_intent();
    let destination = CHAIN_ID;
    let route_bytes: Vec<u8> = route.try_to_vec().unwrap();
    let route_hash = keccak256(&route_bytes);
    let intent_h = intent_hash(destination, &route_hash.into(), &reward.hash());
    let vault_pda = state::vault_pda(&intent_h).0;
    let fund_amount = reward.native_amount;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, fund_amount).unwrap();

    let result = ctx.compact_portal().publish_and_fund(
        destination,
        route.clone(),
        reward.clone(),
        vault_pda,
        true,
        vec![],
    );

    assert!(result
        .clone()
        .is_ok_and(common::contains_event(IntentPublished::new(
            intent_h,
            destination,
            route_bytes,
            reward.clone(),
        ))));
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_h,
        ctx.funder.pubkey(),
        false,
    ))));
    assert_eq!(ctx.balance(&vault_pda), fund_amount);
}

#[test]
fn publish_and_fund_non_solana_destination_uses_abi() {
    let mut ctx = common::Context::default();
    let (_, route, reward) = ctx.rand_intent();
    let destination = 1;
    let route_bytes: Vec<u8> = route_to_abi(route.clone());
    let route_hash = keccak256(&route_bytes);
    let intent_h = intent_hash(destination, &route_hash.into(), &reward.hash());
    let vault_pda = state::vault_pda(&intent_h).0;
    let fund_amount = reward.native_amount;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, fund_amount).unwrap();

    let result = ctx.compact_portal().publish_and_fund(
        destination,
        route.clone(),
        reward.clone(),
        vault_pda,
        true,
        vec![],
    );

    assert!(result
        .clone()
        .is_ok_and(common::contains_event(IntentPublished::new(
            intent_h,
            destination,
            route_bytes,
            reward.clone(),
        ))));
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_h,
        ctx.funder.pubkey(),
        false,
    ))));
    assert_eq!(ctx.balance(&vault_pda), fund_amount);
}
