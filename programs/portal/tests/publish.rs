use portal::events::IntentPublished;
use portal::instructions::PortalError;
use portal::types::intent_hash;
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn publish_intent_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let creator_balance = ctx.balance(&ctx.creator.pubkey());
    let payer_balance = ctx.balance(&ctx.payer.pubkey());

    let result = ctx.publish_intent(&intent, route_hash);
    assert!(
        result.is_ok_and(common::contains_event(IntentPublished::new(
            intent_hash(intent.route_chain, route_hash, &intent.reward),
            intent.route,
            intent.reward,
        )))
    );
    assert_eq!(ctx.balance(&ctx.creator.pubkey()), creator_balance);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
}

#[test]
fn publish_intent_fail_with_wrong_creator() {
    let mut ctx = common::Context::default();
    let mut intent = ctx.rand_intent();
    intent.reward.creator = Pubkey::new_unique();
    let route_hash = [8u8; 32];

    let result = ctx.publish_intent(&intent, route_hash);
    assert!(result.is_err_and(common::is_portal_error(PortalError::InvalidIntentCreator)));
}
