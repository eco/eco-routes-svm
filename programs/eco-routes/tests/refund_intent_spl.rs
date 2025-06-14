use anchor_spl::associated_token::get_associated_token_address_with_program_id;
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

    for token in &intent.reward.tokens {
        ctx.airdrop_token(
            &Pubkey::new_from_array(token.token),
            &ctx.funder.pubkey(),
            token.amount,
        );
    }

    ctx.fund_intent_native(intent.intent_hash).unwrap();

    for token in &intent.reward.tokens {
        let mint = Pubkey::new_from_array(token.token);
        ctx.fund_intent_spl(intent.intent_hash, &mint).unwrap();
    }

    (ctx, intent.intent_hash)
}

#[test]
fn refund_intent_spl_success_from_funded_state() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();

    ctx.expire_intent(intent_hash);

    intent
        .reward
        .tokens
        .iter()
        .enumerate()
        .for_each(|(i, token)| {
            let mint = Pubkey::new_from_array(token.token);
            let creator_token = get_associated_token_address_with_program_id(
                &ctx.creator.pubkey(),
                &mint,
                &anchor_spl::token::spl_token::ID,
            );
            let creator_token_balance = ctx.token_balance(&creator_token);

            let tx = ctx.refund_intent_spl(intent_hash, &mint).unwrap();

            assert_eq!(
                ctx.token_balance(&creator_token) - creator_token_balance,
                token.amount
            );
            let intent: Intent = ctx.account(&intent_pda).unwrap();
            assert_eq!(
                intent.status,
                IntentStatus::Funding(true, (intent.reward.tokens.len() - i - 1) as u8)
            );
            let vault_pda = Pubkey::find_program_address(
                &[b"reward", intent_hash.as_ref(), mint.as_ref()],
                &eco_routes::ID,
            )
            .0;
            let vault = ctx.get_account(&vault_pda).unwrap();
            assert_eq!(vault.lamports, 0);
            assert!(vault.data.is_empty());
            common::assert_contains_event(tx, events::IntentRefundedSpl::new(intent_hash, mint));
        });

    let final_intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(final_intent.status, IntentStatus::Funding(true, 0));
}

#[test]
fn refund_intent_spl_success_from_partial_funding_state() {
    let mut ctx = common::Context::new();
    let intent = ctx.rand_intent();
    ctx.publish_intent(&intent).unwrap();

    for token in &intent.reward.tokens {
        ctx.airdrop_token(
            &Pubkey::new_from_array(token.token),
            &ctx.funder.pubkey(),
            token.amount,
        );
    }

    ctx.fund_intent_native(intent.intent_hash).unwrap();
    let token = intent.reward.tokens.first().unwrap();
    let mint = Pubkey::new_from_array(token.token);
    ctx.fund_intent_spl(intent.intent_hash, &mint).unwrap();

    let intent_pda = Intent::pda(intent.intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();

    let creator_token = get_associated_token_address_with_program_id(
        &ctx.creator.pubkey(),
        &mint,
        &anchor_spl::token::spl_token::ID,
    );
    let creator_token_balance = ctx.token_balance(&creator_token);

    ctx.expire_intent(intent.intent_hash);
    let tx = ctx.refund_intent_spl(intent.intent_hash, &mint).unwrap();

    assert_eq!(
        ctx.token_balance(&creator_token) - creator_token_balance,
        token.amount
    );
    let vault_pda = Pubkey::find_program_address(
        &[b"reward", intent.intent_hash.as_ref(), mint.as_ref()],
        &eco_routes::ID,
    )
    .0;
    let vault = ctx.get_account(&vault_pda).unwrap();
    assert_eq!(vault.lamports, 0);
    assert!(vault.data.is_empty());
    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(intent.status, IntentStatus::Funding(true, 0));
    common::assert_contains_event(tx, events::IntentRefundedSpl::new(intent.intent_hash, mint));
}

#[test]
fn refund_intent_spl_fails_with_nonexistent_intent() {
    let mut ctx = common::Context::new();
    let nonexistent_intent_hash = [99; 32];
    let fake_mint = Keypair::new().pubkey();

    ctx.set_mint_account(&fake_mint);

    let result = ctx.refund_intent_spl(nonexistent_intent_hash, &fake_mint);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(3012))
        )
    }));
}

#[test]
fn refund_intent_spl_fails_when_not_expired() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();

    let token = intent.reward.tokens.first().unwrap();
    let mint = Pubkey::new_from_array(token.token);

    let result = ctx.refund_intent_spl(intent_hash, &mint);
    assert!(result.is_err_and(common::is_eco_routes_error(
        EcoRoutesError::IntentNotExpired
    )));
}

#[test]
fn refund_intent_spl_fails_with_invalid_refundee() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();

    let token = intent.reward.tokens.first().unwrap();
    let mint = Pubkey::new_from_array(token.token);

    ctx.expire_intent(intent_hash);
    ctx.creator = Keypair::new();

    let result = ctx.refund_intent_spl(intent_hash, &mint);
    assert!(result.is_err_and(common::is_eco_routes_error(EcoRoutesError::InvalidRefundee)));
}

#[test]
fn refund_intent_spl_fails_when_token_not_funded() {
    let mut ctx = common::Context::new();
    let intent = ctx.rand_intent();
    ctx.publish_intent(&intent).unwrap();

    let token = intent.reward.tokens.first().unwrap();
    let mint = Pubkey::new_from_array(token.token);

    ctx.fund_intent_native(intent.intent_hash).unwrap();
    ctx.expire_intent(intent.intent_hash);

    let result = ctx.refund_intent_spl(intent.intent_hash, &mint);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(3012))
        )
    }));
}

#[test]
fn refund_intent_spl_fails_with_invalid_token() {
    let (mut ctx, intent_hash) = setup();

    let wrong_mint = Pubkey::new_unique();
    ctx.set_mint_account(&wrong_mint);
    ctx.expire_intent(intent_hash);

    let result = ctx.refund_intent_spl(intent_hash, &wrong_mint);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(3012))
        )
    }));
}
