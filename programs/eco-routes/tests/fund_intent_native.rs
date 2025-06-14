use eco_routes::error::EcoRoutesError;
use eco_routes::events;
use eco_routes::state::{Intent, IntentStatus};
use solana_sdk::instruction::InstructionError;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::TransactionError;

pub mod common;

fn setup() -> (common::Context, [u8; 32]) {
    let mut ctx = common::Context::new();

    let intent = ctx.rand_intent();
    ctx.publish_intent(&intent).unwrap();

    (ctx, intent.intent_hash)
}

#[test]
fn fund_intent_native_success_with_tokens_not_funded() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let funder_balance = ctx.balance(&ctx.funder.pubkey());
    let intent_balance = ctx.balance(&intent_pda);

    let tx = ctx.fund_intent_native(intent_hash).unwrap();

    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(intent.status, IntentStatus::Funding(true, 0));
    assert!(payer_balance > ctx.balance(&ctx.payer.pubkey()));
    assert_eq!(
        funder_balance - ctx.balance(&ctx.funder.pubkey()),
        intent.reward.native_amount
    );
    assert_eq!(
        ctx.balance(&intent_pda) - intent_balance,
        intent.reward.native_amount
    );
    common::assert_contains_event(tx, events::IntentFundedNative::new(intent_hash));
}

#[test]
fn fund_intent_native_success_with_tokens_funded() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let funder_balance = ctx.balance(&ctx.funder.pubkey());
    let intent_balance = ctx.balance(&intent_pda);
    let intent: Intent = ctx.account(&intent_pda).unwrap();

    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        ctx.airdrop_token(&mint, &ctx.funder.pubkey(), token.amount);
        ctx.fund_intent_spl(intent_hash, &mint).unwrap();
    });

    let tx = ctx.fund_intent_native(intent_hash).unwrap();

    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(intent.status, IntentStatus::Funded);
    assert!(payer_balance > ctx.balance(&ctx.payer.pubkey()));
    assert_eq!(
        funder_balance - ctx.balance(&ctx.funder.pubkey()),
        intent.reward.native_amount
    );
    assert_eq!(
        ctx.balance(&intent_pda) - intent_balance,
        intent.reward.native_amount
    );
    common::assert_contains_event(tx, events::IntentFundedNative::new(intent_hash));
}

#[test]
fn fund_intent_native_success_with_intent_partially_funded() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();
    let partial_amount = intent.reward.native_amount / 2;
    ctx.airdrop(&intent_pda, partial_amount).unwrap();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let funder_balance = ctx.balance(&ctx.funder.pubkey());
    let intent_balance = ctx.balance(&intent_pda);

    let tx = ctx.fund_intent_native(intent_hash).unwrap();

    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(intent.status, IntentStatus::Funding(true, 0));
    assert!(payer_balance > ctx.balance(&ctx.payer.pubkey()));
    assert_eq!(
        funder_balance - ctx.balance(&ctx.funder.pubkey()),
        intent.reward.native_amount - partial_amount
    );
    assert_eq!(
        ctx.balance(&intent_pda) - intent_balance,
        intent.reward.native_amount - partial_amount
    );
    common::assert_contains_event(tx, events::IntentFundedNative::new(intent_hash));
}

#[test]
fn fund_intent_native_success_with_intent_fully_funded() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();
    ctx.airdrop(&intent_pda, intent.reward.native_amount)
        .unwrap();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let funder_balance = ctx.balance(&ctx.funder.pubkey());
    let intent_balance = ctx.balance(&intent_pda);

    let tx = ctx.fund_intent_native(intent_hash).unwrap();

    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(intent.status, IntentStatus::Funding(true, 0));
    assert!(payer_balance > ctx.balance(&ctx.payer.pubkey()));
    assert_eq!(funder_balance, ctx.balance(&ctx.funder.pubkey()));
    assert_eq!(intent_balance, ctx.balance(&intent_pda));
    common::assert_contains_event(tx, events::IntentFundedNative::new(intent_hash));
}

#[test]
fn fund_intent_native_fails_with_nonexistent_intent() {
    let mut ctx = common::Context::new();
    let nonexistent_intent_hash = [99; 32];

    let result = ctx.fund_intent_native(nonexistent_intent_hash);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(3012))
        )
    }));
}

#[test]
fn fund_intent_native_fails_when_already_funded() {
    let (mut ctx, intent_hash) = setup();

    ctx.fund_intent_native(intent_hash).unwrap();

    let result = ctx.fund_intent_native(intent_hash);
    assert!(result.is_err_and(common::is_eco_routes_error(
        EcoRoutesError::NotInFundingPhase
    )));
}

#[test]
fn fund_intent_native_fails_with_insufficient_funds() {
    let mut ctx = common::Context::new();
    ctx.funder = Keypair::new();

    let intent = ctx.rand_intent();
    ctx.publish_intent(&intent).unwrap();

    let result = ctx.fund_intent_native(intent.intent_hash);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(1))
        )
    }));
}
