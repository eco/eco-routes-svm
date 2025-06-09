use anchor_spl::{
    associated_token::get_associated_token_address_with_program_id, token::spl_token,
};
use eco_routes::{
    error::EcoRoutesError,
    events,
    state::{Intent, IntentStatus},
};
use solana_sdk::{
    instruction::InstructionError, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::TransactionError,
};

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

    (ctx, intent.intent_hash)
}

#[test]
fn fund_intent_spl_success_with_native_not_funded() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();

    intent
        .reward
        .tokens
        .iter()
        .enumerate()
        .for_each(|(i, token)| {
            let intent: Intent = ctx.account(&intent_pda).unwrap();
            assert_eq!(intent.status, IntentStatus::Funding(false, i as u8));

            let mint = Pubkey::new_from_array(token.token);
            let tx = ctx.fund_intent_spl(intent_hash, &mint).unwrap();

            let funder_token = get_associated_token_address_with_program_id(
                &ctx.funder.pubkey(),
                &mint,
                &spl_token::ID,
            );
            assert_eq!(ctx.token_balance(&funder_token), 0);
            let vault_pda = Pubkey::find_program_address(
                &[b"reward", intent_hash.as_ref(), mint.as_ref()],
                &eco_routes::ID,
            )
            .0;
            assert_eq!(ctx.token_balance(&vault_pda), token.amount);
            common::assert_contains_event(tx, events::IntentFundedSpl::new(intent_hash, mint));
        });

    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(
        intent.status,
        IntentStatus::Funding(false, intent.reward.tokens.len() as u8)
    );
}

#[test]
fn fund_intent_spl_success_with_native_funded() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();

    ctx.fund_intent_native(intent_hash).unwrap();

    intent
        .reward
        .tokens
        .iter()
        .enumerate()
        .for_each(|(i, token)| {
            let intent: Intent = ctx.account(&intent_pda).unwrap();
            assert_eq!(intent.status, IntentStatus::Funding(true, i as u8));

            let mint = Pubkey::new_from_array(token.token);
            let tx = ctx.fund_intent_spl(intent_hash, &mint).unwrap();

            let funder_token = get_associated_token_address_with_program_id(
                &ctx.funder.pubkey(),
                &mint,
                &spl_token::ID,
            );
            assert_eq!(ctx.token_balance(&funder_token), 0);
            let vault_pda = Pubkey::find_program_address(
                &[b"reward", intent_hash.as_ref(), mint.as_ref()],
                &eco_routes::ID,
            )
            .0;
            assert_eq!(ctx.token_balance(&vault_pda), token.amount);
            common::assert_contains_event(tx, events::IntentFundedSpl::new(intent_hash, mint));
        });

    let intent: Intent = ctx.account(&intent_pda).unwrap();
    assert_eq!(intent.status, IntentStatus::Funded);
}

#[test]
fn fund_intent_spl_fails_with_nonexistent_intent() {
    let mut ctx = common::Context::new();
    let nonexistent_intent_hash = [99; 32];
    let mint = Pubkey::new_unique();

    let result = ctx.fund_intent_spl(nonexistent_intent_hash, &mint);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(3012))
        )
    }));
}

#[test]
fn fund_intent_spl_fails_when_token_already_funded() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();
    let token = intent.reward.tokens.first().unwrap();
    let mint = Pubkey::new_from_array(token.token);

    ctx.fund_intent_spl(intent_hash, &mint).unwrap();

    let result = ctx.fund_intent_spl(intent_hash, &mint);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(0))
        )
    }));
}

#[test]
fn fund_intent_spl_fails_with_insufficient_funds() {
    let (mut ctx, intent_hash) = setup();
    let intent_pda = Intent::pda(intent_hash).0;
    let intent: Intent = ctx.account(&intent_pda).unwrap();
    let token = intent.reward.tokens.first().unwrap();
    let mint = Pubkey::new_from_array(token.token);

    ctx.funder = Keypair::new();
    let funder = ctx.funder.pubkey();
    ctx.airdrop_token(&mint, &funder, token.amount - 1);

    let result = ctx.fund_intent_spl(intent.intent_hash, &mint);
    assert!(result.is_err_and(|err| {
        matches!(
            err.err,
            TransactionError::InstructionError(_, InstructionError::Custom(1))
        )
    }));
}

#[test]
fn fund_intent_spl_fails_with_wrong_mint() {
    let (mut ctx, intent_hash) = setup();
    let wrong_mint = Pubkey::new_unique();

    ctx.set_mint_account(&wrong_mint);
    ctx.airdrop_token(&wrong_mint, &ctx.funder.pubkey(), 1_000_000);

    let result = ctx.fund_intent_spl(intent_hash, &wrong_mint);
    assert!(result.is_err_and(common::is_eco_routes_error(EcoRoutesError::InvalidToken)));
}
