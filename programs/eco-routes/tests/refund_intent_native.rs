use eco_routes::{
    error::EcoRoutesError,
    state::{Intent, IntentStatus},
};
use solana_sdk::{instruction::InstructionError, signer::Signer, transaction::TransactionError};

pub mod common;

fn setup() -> (common::Context, [u8; 32]) {
    let mut ctx = common::Context::new();

    let intent = ctx.rand_intent();
    ctx.publish_intent(&intent).unwrap();

    for token in &intent.reward.tokens {
        ctx.airdrop_token(
            &solana_sdk::pubkey::Pubkey::new_from_array(token.token),
            &ctx.funder.pubkey(),
            token.amount,
        );
    }

    ctx.fund_intent_native(intent.intent_hash).unwrap();

    (ctx, intent.intent_hash)
}

#[test]
fn refund_intent_native_success_from_funding_state() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let creator_balance = ctx.balance(&ctx.creator.pubkey());
    let intent_balance = ctx.balance(&intent_pda);

    ctx.expire_intent(intent_hash);
    ctx.refund_intent_native(intent_hash).unwrap();

    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(intent.status, IntentStatus::Funding(false, 0));
    assert_eq!(
        ctx.balance(&ctx.creator.pubkey()) - creator_balance,
        intent.reward.native_amount
    );
    assert_eq!(
        intent_balance - ctx.balance(&intent_pda),
        intent.reward.native_amount
    );
}

#[test]
fn refund_intent_native_success_from_funded_state() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();
    let creator_balance = ctx.balance(&ctx.creator.pubkey());
    let intent_balance = ctx.balance(&intent_pda);

    intent.reward.tokens.iter().for_each(|token| {
        let mint = solana_sdk::pubkey::Pubkey::new_from_array(token.token);
        ctx.fund_intent_spl(intent_hash, &mint).unwrap();
    });
    ctx.expire_intent(intent_hash);
    ctx.refund_intent_native(intent_hash).unwrap();

    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(
        intent.status,
        IntentStatus::Funding(false, intent.reward.tokens.len() as u8)
    );
    assert_eq!(
        ctx.balance(&ctx.creator.pubkey()) - creator_balance,
        intent.reward.native_amount
    );
    assert_eq!(
        intent_balance - ctx.balance(&intent_pda),
        intent.reward.native_amount
    );
}

#[test]
fn refund_intent_native_fails_with_nonexistent_intent() {
    let mut ctx = common::Context::new();
    let nonexistent_intent_hash = [99; 32];

    let result = ctx.refund_intent_native(nonexistent_intent_hash);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(3012))
        )
    }));
}

#[test]
fn refund_intent_native_fails_when_not_expired() {
    let (mut ctx, intent_hash) = setup();

    let result = ctx.refund_intent_native(intent_hash);
    assert!(result.is_err_and(common::is_eco_routes_error(
        EcoRoutesError::IntentNotExpired
    )));
}

#[test]
fn refund_intent_native_fails_with_invalid_refundee() {
    let (mut ctx, intent_hash) = setup();

    ctx.expire_intent(intent_hash);
    ctx.creator = solana_sdk::signature::Keypair::new();

    let result = ctx.refund_intent_native(intent_hash);
    assert!(result.is_err_and(common::is_eco_routes_error(EcoRoutesError::InvalidRefundee)));
}

#[test]
fn refund_intent_native_fails_when_native_not_funded() {
    let mut ctx = common::Context::new();
    let intent = ctx.rand_intent();

    ctx.publish_intent(&intent).unwrap();
    ctx.expire_intent(intent.intent_hash);

    let result = ctx.refund_intent_native(intent.intent_hash);
    assert!(result.is_err_and(common::is_eco_routes_error(
        EcoRoutesError::NotInRefundingPhase
    )));
}
