use anchor_lang::AnchorSerialize;
use portal::{events::IntentPublished, types::intent_hash};
use solana_sdk::signer::Signer;
use tiny_keccak::{Hasher, Keccak};

pub mod common;

#[test]
fn publish_intent_success() {
    let mut ctx = common::Context::default();
    let (destination, route, reward) = ctx.rand_intent();
    let route: Vec<u8> = route.try_to_vec().unwrap();
    let creator_balance = ctx.balance(&ctx.creator.pubkey());
    let payer_balance = ctx.balance(&ctx.payer.pubkey());

    let mut hasher = Keccak::v256();
    let mut route_hash = [0u8; 32];
    hasher.update(&route);
    hasher.finalize(&mut route_hash);

    let result = ctx
        .portal()
        .publish_intent(destination, route.clone(), reward.clone());
    assert!(
        result.is_ok_and(common::contains_event(IntentPublished::new(
            intent_hash(destination, &route_hash.into(), &reward.hash()),
            destination,
            route,
            reward,
        )))
    );
    assert_eq!(ctx.balance(&ctx.creator.pubkey()), creator_balance);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
}
