use eco_routes::state::Intent;
use solana_sdk::{instruction::InstructionError, signer::Signer, transaction::TransactionError};

pub mod common;

#[test]
fn publish_intent_success() {
    let mut ctx = common::Context::new();
    let intent = ctx.rand_intent();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let creator_balance = ctx.balance(&ctx.creator.pubkey());

    ctx.publish_intent(&intent).unwrap();

    let actual: Intent = ctx.account(&Intent::pda(intent.intent_hash).0);
    assert_eq!(actual, intent);
    assert!(payer_balance > ctx.balance(&ctx.payer.pubkey()));
    assert_eq!(creator_balance, ctx.balance(&ctx.creator.pubkey()));
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
