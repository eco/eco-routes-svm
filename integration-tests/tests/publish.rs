use portal::events::IntentPublished;
use portal::types::intent_hash;
use rand::random;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn publish_intent_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random::<[u8; 32]>().into();
    let creator_balance = ctx.balance(&ctx.creator.pubkey());
    let payer_balance = ctx.balance(&ctx.payer.pubkey());

    let result = ctx.portal().publish_intent(&intent, route_hash);
    assert!(
        result.is_ok_and(common::contains_event(IntentPublished::new(
            intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash()),
            intent.route,
            intent.reward,
        )))
    );
    assert_eq!(ctx.balance(&ctx.creator.pubkey()), creator_balance);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
}
