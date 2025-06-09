use eco_routes::{events, state::Intent};
use solana_sdk::{instruction::InstructionError, signer::Signer, transaction::TransactionError};

pub mod common;

#[test]
fn publish_intent_success() {
    let mut ctx = common::Context::new();
    let intent = ctx.rand_intent();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let creator_balance = ctx.balance(&ctx.creator.pubkey());

    let tx = ctx.publish_intent(&intent).unwrap();

    let actual: Intent = ctx.account(&Intent::pda(intent.intent_hash).0).unwrap();
    assert_eq!(actual, intent);
    assert!(payer_balance > ctx.balance(&ctx.payer.pubkey()));
    assert_eq!(creator_balance, ctx.balance(&ctx.creator.pubkey()));
    common::assert_contains_event(
        tx,
        events::IntentCreated::new(
            intent.intent_hash,
            intent.route.clone(),
            intent.reward.clone(),
        ),
    );
}

#[test]
fn publish_intent_duplicate_fails() {
    let mut ctx = common::Context::new();
    let intent = ctx.rand_intent();

    ctx.publish_intent(&intent).unwrap();

    let actual = ctx.publish_intent(&intent);
    assert!(actual.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(0))
        )
    }));
}
