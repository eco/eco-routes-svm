use std::iter;

use anchor_lang::prelude::AccountMeta;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::Proof;
use eco_svm_std::Bytes32;
use hyper_prover::state::pda_payer_pda;
use portal::events::IntentRefunded;
use portal::state::{self, proof_closer_pda};
use portal::types::{intent_hash, Reward};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

fn setup(is_token_2022: bool) -> (common::Context, u64, Reward, Bytes32) {
    let mut ctx = if is_token_2022 {
        common::Context::new_with_token_2022()
    } else {
        common::Context::default()
    };
    let (destination, _, reward) = ctx.rand_intent();
    let route_hash = random::<[u8; 32]>().into();
    let funder = ctx.funder.pubkey();
    let vault_pda = state::vault_pda(&intent_hash(destination, &route_hash, &reward.hash())).0;
    let token_program = &ctx.token_program.clone();

    ctx.airdrop(&funder, reward.native_amount).unwrap();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &funder, token.amount);
    });

    ctx.portal()
        .fund_intent(
            destination,
            reward.clone(),
            vault_pda,
            route_hash,
            false,
            reward.tokens.iter().flat_map(|token| {
                let funder_token = get_associated_token_address_with_program_id(
                    &funder,
                    &token.token,
                    token_program,
                );
                let vault_ata = get_associated_token_address_with_program_id(
                    &vault_pda,
                    &token.token,
                    token_program,
                );

                vec![
                    AccountMeta::new(funder_token, false),
                    AccountMeta::new(vault_ata, false),
                    AccountMeta::new_readonly(token.token, false),
                ]
            }),
        )
        .unwrap();

    (ctx, destination, reward, route_hash)
}

#[test]
fn refund_intent_native_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let creator = reward.creator;
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        vec![],
    );
    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert_eq!(ctx.balance(&vault), 0);
}

#[test]
fn refund_intent_tokens_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let creator = reward.creator;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &creator, 0);
    });
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            let creator_token =
                get_associated_token_address_with_program_id(&creator, &token.token, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(creator_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();
    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        token_accounts,
    );
    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        creator,
    ))));
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &creator), token.amount);
    });
}

#[test]
fn refund_intent_tokens_2022_success() {
    let (mut ctx, destination, reward, route_hash) = setup(true);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let creator = reward.creator;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &creator, 0);
    });
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            let creator_token =
                get_associated_token_address_with_program_id(&creator, &token.token, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(creator_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();
    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        token_accounts,
    );
    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        creator,
    ))));
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &creator), token.amount);
    });
}

#[test]
fn refund_intent_native_and_token_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &creator, 0);
    });
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            let creator_token =
                get_associated_token_address_with_program_id(&creator, &token.token, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(creator_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();
    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        token_accounts,
    );
    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        creator,
    ))));
    assert_eq!(ctx.balance(&vault), 0);
    assert_eq!(ctx.balance(&creator), reward.native_amount);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &creator), token.amount);
    });
}

#[test]
fn refund_intent_fulfilled_on_wrong_chain_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    let fulfillment_proof = Proof::new(random(), Pubkey::new_unique());
    ctx.set_proof(proof, fulfillment_proof, hyper_prover::ID);
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        vec![],
    );
    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        creator,
    ))));
    assert_eq!(ctx.balance(&creator), reward.native_amount);
    assert_eq!(ctx.balance(&vault), 0);
}

#[test]
fn refund_intent_withdrawn_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    let fulfillment_proof = Proof::new(destination, Pubkey::new_unique());
    ctx.set_proof(proof, fulfillment_proof, hyper_prover::ID);
    ctx.set_withdrawn_marker(withdrawn_marker);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        vec![],
    );
    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        creator,
    ))));
    assert_eq!(ctx.balance(&creator), reward.native_amount);
    assert_eq!(ctx.balance(&vault), 0);
}

#[test]
fn refund_intent_invalid_creator_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let wrong_creator = Pubkey::new_unique();
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        wrong_creator,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidCreator
    )));
}

#[test]
fn refund_intent_invalid_vault_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let wrong_vault = Pubkey::new_unique();
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        wrong_vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidVault
    )));
}

#[test]
fn refund_intent_invalid_proof_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let wrong_proof = Pubkey::new_unique();
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        wrong_proof,
        withdrawn_marker,
        creator,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidProof
    )));
}

#[test]
fn refund_intent_fulfilled_and_not_withdrawn_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    let fulfillment_proof = Proof::new(destination, Pubkey::new_unique());
    ctx.set_proof(proof, fulfillment_proof, hyper_prover::ID);
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentFulfilledAndNotWithdrawn
    )));
}

#[test]
fn refund_intent_not_expired_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::RewardNotExpired
    )));
}

#[test]
fn refund_intent_invalid_creator_token_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let wrong_owner = Pubkey::new_unique();
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &wrong_owner, 0);
    });
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            let wrong_owner_token = get_associated_token_address_with_program_id(
                &wrong_owner,
                &token.token,
                token_program,
            );
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(wrong_owner_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();
    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        token_accounts,
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidCreatorToken
    )));
}

#[test]
fn refund_intent_after_withdraw_excessive_funding_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let creator = reward.creator;
    let claimant = Pubkey::new_unique();
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.airdrop(&vault, 50_000).unwrap();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &vault, 1000);
    });
    ctx.set_proof(proof, Proof::new(destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
        ctx.airdrop_token_ata(&token.token, &creator, 0);
    });

    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            let claimant_token = get_associated_token_address_with_program_id(
                &claimant,
                &token.token,
                token_program,
            );
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(claimant_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();
    ctx.portal()
        .withdraw_intent(
            destination,
            reward.clone(),
            vault,
            route_hash,
            claimant,
            proof,
            withdrawn_marker,
            proof_closer_pda().0,
            token_accounts,
            iter::once(AccountMeta::new(pda_payer_pda().0, false)),
        )
        .unwrap();
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            let creator_token =
                get_associated_token_address_with_program_id(&creator, &token.token, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(creator_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();
    let result = ctx.portal().refund_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        creator,
        token_accounts,
    );
    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        creator,
    ))));
    assert_eq!(ctx.balance(&creator), 50_000);
    assert_eq!(ctx.balance(&vault), 0);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &creator), 1000);
    });
}
