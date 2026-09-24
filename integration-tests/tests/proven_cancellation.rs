use anchor_lang::prelude::AccountMeta;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::{IntentProven, Proof};
use eco_svm_std::{Bytes32, CANCELLED, CHAIN_ID};
use local_prover::state::ProofAccount;
use portal::events::IntentRefunded;
use portal::instructions::PortalError;
use portal::state::{self, proof_closer_pda, FulfillMarker, FulfillTombstone, WithdrawnMarker};
use portal::types::{self, Reward, Route};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

/// Every test transaction here carries one signature, and LiteSVM's default
/// fee structure charges 5000 lamports each.
const TRANSACTION_FEE: u64 = 5_000;

/// `[from ATA, to ATA, mint]` for every reward token, the triple layout
/// `fund`, `withdraw` and `refund` all take.
fn reward_token_accounts(
    ctx: &common::Context,
    reward: &Reward,
    from: &Pubkey,
    to: &Pubkey,
) -> Vec<AccountMeta> {
    reward
        .tokens
        .iter()
        .flat_map(|token| {
            vec![
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        from,
                        &token.token,
                        &ctx.token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        to,
                        &token.token,
                        &ctx.token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect()
}

/// A same-chain intent proven by local-prover, cancelled on this chain and
/// proven back to it: the full SVM round trip. Funded with native reward, plus
/// the reward tokens when `with_reward_tokens` is set.
fn cancelled_and_proven_intent(
    ctx: &mut common::Context,
    with_reward_tokens: bool,
) -> (Route, Reward, Bytes32) {
    let (_, route, mut reward) = ctx.rand_minimal_intent(local_prover::ID);
    if !with_reward_tokens {
        reward.tokens.clear();
    }
    let route_hash = route.hash();
    let intent_hash = types::intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, reward.native_amount).unwrap();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &funder, token.amount);
    });
    let funding_accounts = reward_token_accounts(ctx, &reward, &funder, &vault);
    ctx.portal()
        .fund_intent(
            CHAIN_ID,
            reward.clone(),
            vault,
            route_hash,
            false,
            funding_accounts,
        )
        .unwrap();

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward.hash(), fulfill_marker)
        .unwrap();
    ctx.portal()
        .prove_intent_via_local_prover(
            vec![intent_hash],
            CHAIN_ID,
            vec![fulfill_marker],
            state::dispatcher_pda(&local_prover::ID).0,
            vec![Proof::pda(&intent_hash, &local_prover::ID).0],
        )
        .unwrap();

    (route, reward, intent_hash)
}

#[test]
fn cancel_prove_refund_via_local_prover_before_reward_deadline_success() {
    let mut ctx = common::Context::default();
    let (route, reward, intent_hash) = cancelled_and_proven_intent(&mut ctx, false);
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
    let payer = ctx.payer.pubkey();
    let payer_balance = ctx.balance(&payer);
    let proof_rent = ctx.balance(&proof);

    assert!(ctx.now() < reward.deadline);

    let result = ctx.portal().refund_intent_with_close_proof(
        CHAIN_ID,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route.hash(),
        proof,
        WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
        vec![AccountMeta::new(payer, true)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert!(ctx.get_account(&proof).is_none());
    assert_eq!(
        ctx.balance(&payer),
        payer_balance + proof_rent - TRANSACTION_FEE
    );
}

/// The real fast path always sweeps tokens alongside the close-proof tail:
/// the creator is paid, the vault ATAs are closed and the proof is closed in
/// the same refund.
#[test]
fn cancel_prove_refund_tokens_via_local_prover_before_reward_deadline_success() {
    let mut ctx = common::Context::default();
    let (route, reward, intent_hash) = cancelled_and_proven_intent(&mut ctx, true);
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
    let creator = reward.creator;
    let payer = ctx.payer.pubkey();

    assert!(!reward.tokens.is_empty());
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &creator, 0);
    });
    let vault_atas: Vec<_> = reward
        .tokens
        .iter()
        .map(|token| {
            get_associated_token_address_with_program_id(&vault, &token.token, &ctx.token_program)
        })
        .collect();
    let vault_ata_rent: u64 = vault_atas.iter().map(|ata| ctx.balance(ata)).sum();
    let payer_balance = ctx.balance(&payer);
    let proof_rent = ctx.balance(&proof);
    let token_accounts = reward_token_accounts(&ctx, &reward, &vault, &creator);

    assert!(ctx.now() < reward.deadline);

    let result = ctx.portal().refund_intent_with_close_proof(
        CHAIN_ID,
        reward.clone(),
        vault,
        route.hash(),
        proof,
        WithdrawnMarker::pda(&intent_hash).0,
        creator,
        token_accounts,
        vec![AccountMeta::new(payer, true)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        creator,
    ))));
    assert_eq!(ctx.balance(&creator), reward.native_amount);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &creator), token.amount);
    });
    vault_atas.iter().for_each(|ata| {
        assert!(ctx.get_account(ata).is_none());
    });
    assert!(ctx.get_account(&proof).is_none());
    assert_eq!(
        ctx.balance(&payer),
        payer_balance + proof_rent + vault_ata_rent - TRANSACTION_FEE
    );
}

#[test]
fn cancel_prove_withdraw_via_local_prover_fail() {
    let mut ctx = common::Context::default();
    let (route, reward, intent_hash) = cancelled_and_proven_intent(&mut ctx, false);
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
    let cancelled = Pubkey::new_from_array(CANCELLED.into());
    let payer = ctx.payer.pubkey();

    let result = ctx.portal().withdraw_intent(
        CHAIN_ID,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route.hash(),
        cancelled,
        proof,
        WithdrawnMarker::pda(&intent_hash).0,
        proof_closer_pda(&local_prover::ID).0,
        vec![],
        vec![AccountMeta::new(payer, true)],
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentCancelled)));
    assert_eq!(ctx.balance(&cancelled), 0);
    assert!(ctx.get_account(&proof).is_some());
}

/// P0: every account a successful payout to `CANCELLED` would need is
/// supplied, including existing `CANCELLED` ATAs, so only the
/// `IntentCancelled` guard stands between the vault and that key.
#[test]
fn cancel_prove_withdraw_tokens_via_local_prover_fail() {
    let mut ctx = common::Context::default();
    let (route, reward, intent_hash) = cancelled_and_proven_intent(&mut ctx, true);
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
    let cancelled = Pubkey::new_from_array(CANCELLED.into());
    let payer = ctx.payer.pubkey();

    assert!(!reward.tokens.is_empty());
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &cancelled, 0);
    });
    let vault_balance = ctx.balance(&vault);
    let token_accounts = reward_token_accounts(&ctx, &reward, &vault, &cancelled);

    let result = ctx.portal().withdraw_intent(
        CHAIN_ID,
        reward.clone(),
        vault,
        route.hash(),
        cancelled,
        proof,
        WithdrawnMarker::pda(&intent_hash).0,
        proof_closer_pda(&local_prover::ID).0,
        token_accounts,
        vec![AccountMeta::new(payer, true)],
    );

    assert!(result.is_err_and(common::is_error(PortalError::IntentCancelled)));
    assert_eq!(ctx.balance(&cancelled), 0);
    assert_eq!(ctx.balance(&vault), vault_balance);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), token.amount);
        assert_eq!(ctx.token_balance_ata(&token.token, &cancelled), 0);
    });
    assert!(ctx.get_account(&proof).is_some());
    assert!(ctx
        .get_account(&WithdrawnMarker::pda(&intent_hash).0)
        .is_none());
}

/// Closing a cancelled marker leaves a tombstone that still proves the
/// cancellation.
#[test]
fn cancel_close_marker_prove_via_local_prover_success() {
    let mut ctx = common::Context::default();
    let (intent_hash, route, reward) = ctx.rand_minimal_intent(local_prover::ID);
    let fulfill_marker = FulfillMarker::pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &local_prover::ID).0;
    let cancelled = Pubkey::new_from_array(CANCELLED.into());

    ctx.warp_to_timestamp(route.deadline as i64 + 1);
    ctx.portal()
        .cancel_intent(intent_hash, &route, reward.hash(), fulfill_marker)
        .unwrap();
    ctx.portal()
        .close_fulfill_marker(intent_hash, fulfill_marker)
        .unwrap();
    assert!(ctx.account::<FulfillTombstone>(&fulfill_marker).is_some());

    let result = ctx.portal().prove_intent_via_local_prover(
        vec![intent_hash],
        CHAIN_ID,
        vec![fulfill_marker],
        state::dispatcher_pda(&local_prover::ID).0,
        vec![proof],
    );

    assert!(
        result.is_ok_and(common::contains_cpi_event(IntentProven::new(
            intent_hash,
            cancelled,
            CHAIN_ID,
        )))
    );
    let proof: ProofAccount = ctx.account(&proof).unwrap();
    assert_eq!(CHAIN_ID, proof.0.destination);
    assert!(CANCELLED == proof.0.claimant);
}
