use std::iter;

use anchor_lang::prelude::AccountMeta;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CANCELLED};
use hyper_prover::state::pda_payer_pda;
use portal::events::IntentRefunded;
use portal::state;
use portal::types::{intent_hash, Reward};
use rand::random;
use solana_address_lookup_table_interface::instruction::{
    create_lookup_table, extend_lookup_table,
};
use solana_sdk::message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::slot_hashes::SlotHashes;
use solana_sdk::transaction::{Transaction, VersionedTransaction};

pub mod common;

fn setup_with_prover(
    is_token_2022: bool,
    prover: Pubkey,
) -> (common::Context, u64, Reward, Bytes32) {
    setup_with_reward(is_token_2022, prover, |_, _| {})
}

/// `shape_reward` adjusts the random reward before it is funded.
fn setup_with_reward(
    is_token_2022: bool,
    prover: Pubkey,
    shape_reward: impl FnOnce(&mut common::Context, &mut Reward),
) -> (common::Context, u64, Reward, Bytes32) {
    let mut ctx = if is_token_2022 {
        common::Context::new_with_token_2022()
    } else {
        common::Context::default()
    };
    let (destination, _, mut reward) = ctx.rand_intent();
    reward.prover = prover;
    shape_reward(&mut ctx, &mut reward);
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

fn setup(is_token_2022: bool) -> (common::Context, u64, Reward, Bytes32) {
    setup_with_prover(is_token_2022, hyper_prover::ID)
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Withdrawn,
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Expired,
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
        eco_svm_std::prover::ProverError::InvalidProof
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Expired,
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
        portal::instructions::RefundKind::Expired,
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

    // rent-exempt native excess: withdraw leaves it in the vault, refund returns
    // it to the creator (sub-rent excess would instead drain to the claimant)
    let excess = 1_000_000;
    ctx.airdrop(&vault, excess).unwrap();
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
        portal::instructions::RefundKind::Withdrawn,
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
    assert_eq!(ctx.balance(&creator), excess);
    assert_eq!(ctx.balance(&vault), 0);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &creator), 1000);
    });
}

fn cancelled_proof(destination: u64) -> Proof {
    Proof::new(destination, Pubkey::new_from_array(CANCELLED.into()))
}

/// `[vault ATA, creator ATA, mint]` for each of `tokens`, creating the
/// creator's ATAs the refund pays into.
fn refund_token_accounts(
    ctx: &mut common::Context,
    tokens: &[portal::types::TokenAmount],
    vault: &Pubkey,
    creator: &Pubkey,
) -> Vec<AccountMeta> {
    let token_program = ctx.token_program;

    tokens
        .iter()
        .flat_map(|token| {
            ctx.airdrop_token_ata(&token.token, creator, 0);

            vec![
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        vault,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        creator,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect()
}

#[test]
fn refund_intent_cancelled_before_deadline_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let pda_payer = pda_payer_pda().0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    let pda_payer_balance = ctx.balance(&pda_payer);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);
    assert!(ctx.now() < reward.deadline);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        reward.creator,
        token_accounts,
        vec![AccountMeta::new(pda_payer, false)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(
            ctx.token_balance_ata(&token.token, &reward.creator),
            token.amount
        );
    });
    assert!(ctx.get_account(&proof).is_some());
    assert_eq!(ctx.balance(&pda_payer), pda_payer_balance);
}

/// The all-mints rule with a single reward mint: sweeping it is sufficient.
#[test]
fn refund_intent_cancelled_single_token_before_deadline_success() {
    let (mut ctx, destination, reward, route_hash) =
        setup_with_reward(false, hyper_prover::ID, |_, reward| {
            reward.tokens.truncate(1)
        });
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let token = reward.tokens[0].clone();

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);
    assert!(ctx.now() < reward.deadline);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        token_accounts,
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert_eq!(ctx.balance(&vault), 0);
    assert_eq!(
        ctx.token_balance_ata(&token.token, &reward.creator),
        token.amount
    );
    assert!(ctx.get_account(&proof).is_some());
}

/// The cancellation refund consumes the proof, so a second refund before
/// `reward.deadline` finds no proof and falls back to the deadline: nothing
/// moves twice.
#[test]
fn refund_intent_cancelled_retry_still_requires_reward_mints() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);
    ctx.portal()
        .refund_cancelled_intent(
            destination,
            reward.clone(),
            vault,
            route_hash,
            proof,
            withdrawn_marker,
            reward.creator,
            token_accounts,
            vec![AccountMeta::new(pda_payer_pda().0, false)],
        )
        .unwrap();
    let creator_balance = ctx.balance(&reward.creator);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        reward.creator,
        vec![],
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidMint
    )));
    assert_eq!(ctx.balance(&reward.creator), creator_balance);
    assert!(ctx.get_account(&proof).is_some());
    assert!(ctx.get_account(&withdrawn_marker).is_none());
}

/// A cancellation proven for another destination is not this intent's
/// cancellation: the fallback deadline still applies.
#[test]
fn refund_intent_cancelled_on_wrong_destination_not_expired_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    ctx.set_proof(proof, cancelled_proof(destination + 1), hyper_prover::ID);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentNotCancelled
    )));
}

#[test]
fn refund_intent_cancelled_without_cleanup_accounts_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    let vault = state::vault_pda(&intent_hash).0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        token_accounts,
        vec![],
    );

    assert!(result.is_ok());
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert!(ctx.get_account(&proof).is_some());
}

#[test]
fn refund_intent_expired_with_non_program_prover_success() {
    let (mut ctx, destination, reward, route_hash) = setup_with_prover(false, Pubkey::new_unique());
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());

    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent(
        portal::instructions::RefundKind::Expired,
        destination,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route_hash,
        Proof::pda(&intent_hash, &reward.prover).0,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
    );

    assert!(result.is_ok());
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
}

/// A `prover` that is passed must be executable, even on the timeout path that
/// never calls it; omitting it (the portal ID in its slot) still refunds.
#[test]
fn refund_intent_non_program_prover_rejects_cancellation_but_allows_timeout() {
    let (mut ctx, destination, reward, route_hash) = setup_with_prover(false, Pubkey::new_unique());
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());

    ctx.warp_to_timestamp(reward.deadline as i64 + 1);

    let result = ctx.portal().refund_intent_with_accounts(
        destination,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route_hash,
        Proof::pda(&intent_hash, &reward.prover).0,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        portal::instructions::RefundKind::Cancelled,
        Some(reward.prover),
        vec![],
        vec![],
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidProver
    )));
    assert_eq!(ctx.balance(&reward.creator), 0);

    let result = ctx.portal().refund_intent(
        portal::instructions::RefundKind::Expired,
        destination,
        reward.clone(),
        state::vault_pda(&intent_hash).0,
        route_hash,
        Proof::pda(&intent_hash, &reward.prover).0,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
    );

    assert!(result.is_ok());
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
}

#[test]
fn refund_intent_cancelled_missing_reward_mint_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    assert!(reward.tokens.len() > 1);

    for tokens in [&reward.tokens[..0], &reward.tokens[1..]] {
        let token_accounts = refund_token_accounts(&mut ctx, tokens, &vault, &reward.creator);

        let result = ctx.portal().refund_cancelled_intent(
            destination,
            reward.clone(),
            vault,
            route_hash,
            proof,
            state::WithdrawnMarker::pda(&intent_hash).0,
            reward.creator,
            token_accounts,
            vec![AccountMeta::new(pda_payer_pda().0, false)],
        );

        assert!(result.is_err_and(common::is_error(
            portal::instructions::PortalError::InvalidMint
        )));
        assert!(ctx.get_account(&proof).is_some());
        assert_eq!(ctx.balance(&reward.creator), 0);
        assert_eq!(ctx.balance(&vault), reward.native_amount);
        reward.tokens.iter().for_each(|token| {
            assert_eq!(ctx.token_balance_ata(&token.token, &vault), token.amount);
        });
    }
}

#[test]
fn refund_intent_cancelled_non_program_prover_fail() {
    let (mut ctx, destination, reward, route_hash) = setup_with_prover(false, Pubkey::new_unique());
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    ctx.set_proof(proof, cancelled_proof(destination), reward.prover);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);

    for now in [ctx.now(), reward.deadline + 1] {
        ctx.warp_to_timestamp(now as i64);

        let result = ctx.portal().refund_cancelled_intent(
            destination,
            reward.clone(),
            vault,
            route_hash,
            proof,
            state::WithdrawnMarker::pda(&intent_hash).0,
            reward.creator,
            token_accounts.clone(),
            vec![AccountMeta::new(pda_payer_pda().0, false)],
        );

        assert!(result.is_err_and(common::is_error(
            portal::instructions::PortalError::InvalidProver
        )));
        assert!(ctx.get_account(&proof).is_some());
        assert_eq!(ctx.balance(&reward.creator), 0);
    }
}

#[test]
fn refund_intent_cancelled_after_deadline_preserves_proof_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let pda_payer = pda_payer_pda().0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);
    let pda_payer_balance = ctx.balance(&pda_payer);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        vec![],
        vec![AccountMeta::new(pda_payer, false)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert!(ctx.get_account(&proof).is_some());
    assert_eq!(ctx.balance(&pda_payer), pda_payer_balance);
}

#[test]
fn refund_intent_cancelled_after_deadline_without_cleanup_accounts_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    ctx.warp_to_timestamp(reward.deadline as i64);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        token_accounts,
        vec![],
    );

    assert!(result.is_ok());
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert!(ctx.get_account(&proof).is_some());
}

#[test]
fn refund_intent_cancelled_unsweepable_reward_mint_until_deadline_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let missing_vault_ata = get_associated_token_address_with_program_id(
        &vault,
        &reward.tokens[0].token,
        &ctx.token_program,
    );

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    ctx.set_account(missing_vault_ata, Default::default())
        .unwrap();
    assert!(ctx.get_account(&missing_vault_ata).is_none());
    let token_accounts =
        refund_token_accounts(&mut ctx, &reward.tokens[1..], &vault, &reward.creator);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        token_accounts.clone(),
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidMint
    )));
    assert!(ctx.get_account(&proof).is_some());
    assert_eq!(ctx.balance(&vault), reward.native_amount);
    reward.tokens[1..].iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), token.amount);
    });

    ctx.warp_to_timestamp(reward.deadline as i64 + 1);
    let pda_payer_balance = ctx.balance(&pda_payer_pda().0);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        token_accounts,
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert!(ctx.get_account(&proof).is_some());
    assert_eq!(ctx.balance(&pda_payer_pda().0), pda_payer_balance);
    reward.tokens[1..].iter().for_each(|token| {
        assert_eq!(
            ctx.token_balance_ata(&token.token, &reward.creator),
            token.amount
        );
    });
}

/// `prover` must be `reward.prover` whenever it is passed, on the cancelled
/// path and on a timeout refund alike.
#[test]
fn refund_intent_invalid_prover_fail() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);

    for cancelled in [true, false] {
        if !cancelled {
            ctx.set_account(proof, Default::default()).unwrap();
            ctx.warp_to_timestamp(reward.deadline as i64 + 1);
        }

        let result = ctx.portal().refund_intent_with_accounts(
            destination,
            reward.clone(),
            vault,
            route_hash,
            proof,
            state::WithdrawnMarker::pda(&intent_hash).0,
            reward.creator,
            portal::instructions::RefundKind::Cancelled,
            Some(local_prover::ID),
            token_accounts.clone(),
            vec![AccountMeta::new(pda_payer_pda().0, false)],
        );

        assert!(result.is_err_and(common::is_error(
            portal::instructions::PortalError::InvalidProver
        )));
        assert_eq!(ctx.balance(&reward.creator), 0);
    }
}

fn assert_cancelled_fast_refund_success(
    ctx: &mut common::Context,
    destination: u64,
    reward: &Reward,
    route_hash: Bytes32,
    token_accounts: Vec<AccountMeta>,
) {
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    assert!(ctx.now() < reward.deadline);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        token_accounts,
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    assert_eq!(ctx.balance(&vault), 0);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(
            ctx.token_balance_ata(&token.token, &reward.creator),
            token.amount
        );
    });
    assert!(ctx.get_account(&proof).is_some());
}

/// The all-mints check derives vault ATAs under each mint's own token
/// program, so Token-2022 rewards pass it too.
#[test]
fn refund_intent_cancelled_tokens_2022_before_deadline_success() {
    let (mut ctx, destination, reward, route_hash) = setup(true);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;

    assert_eq!(ctx.token_program, anchor_spl::token_2022::ID);
    ctx.set_proof(
        Proof::pda(&intent_hash, &reward.prover).0,
        cancelled_proof(destination),
        hyper_prover::ID,
    );
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);

    assert_cancelled_fast_refund_success(
        &mut ctx,
        destination,
        &reward,
        route_hash,
        token_accounts,
    );
}

/// A zero-amount reward entry is still a reward mint, so its (empty) vault
/// ATA must be swept too.
#[test]
fn refund_intent_cancelled_zero_amount_reward_token_before_deadline_success() {
    let (mut ctx, destination, reward, route_hash) =
        setup_with_reward(false, hyper_prover::ID, |_, reward| {
            reward.tokens[0].amount = 0
        });
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;

    ctx.set_proof(proof, cancelled_proof(destination), hyper_prover::ID);
    assert_eq!(ctx.token_balance_ata(&reward.tokens[0].token, &vault), 0);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);

    let result = ctx.portal().refund_cancelled_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        state::WithdrawnMarker::pda(&intent_hash).0,
        reward.creator,
        token_accounts[3..].to_vec(),
        vec![AccountMeta::new(pda_payer_pda().0, false)],
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidMint
    )));
    assert!(ctx.get_account(&proof).is_some());

    assert_cancelled_fast_refund_success(
        &mut ctx,
        destination,
        &reward,
        route_hash,
        token_accounts,
    );
}

/// A non-reward token sitting in the vault may be swept alongside the reward
/// mints; it neither satisfies nor breaks the all-mints check.
#[test]
fn refund_intent_cancelled_extra_token_before_deadline_success() {
    let (mut ctx, destination, reward, route_hash) = setup(false);
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let extra = portal::types::TokenAmount {
        token: Pubkey::new_unique(),
        amount: 1_000,
    };

    ctx.set_mint_account(&extra.token);
    ctx.airdrop_token_ata(&extra.token, &vault, extra.amount);
    ctx.set_proof(
        Proof::pda(&intent_hash, &reward.prover).0,
        cancelled_proof(destination),
        hyper_prover::ID,
    );
    let token_accounts = refund_token_accounts(
        &mut ctx,
        &[reward.tokens.clone(), vec![extra.clone()]].concat(),
        &vault,
        &reward.creator,
    );

    assert_cancelled_fast_refund_success(
        &mut ctx,
        destination,
        &reward,
        route_hash,
        token_accounts,
    );
    assert_eq!(
        ctx.token_balance_ata(&extra.token, &reward.creator),
        extra.amount
    );
}

/// Solana's wire limit for a serialized legacy or v0 transaction.
const PACKET_DATA_SIZE: usize = 1232;

fn serialized_len(transaction: &Transaction) -> usize {
    bincode::serialize(transaction).unwrap().len()
}

#[test]
fn refund_intent_five_mint_timeout_uses_lookup_table() {
    let (mut ctx, destination, reward, route_hash) =
        setup_with_reward(false, hyper_prover::ID, |ctx, reward| {
            (0..3).for_each(|_| {
                let token = Pubkey::new_unique();
                ctx.set_mint_account(&token);
                reward.tokens.push(portal::types::TokenAmount {
                    token,
                    amount: 1_000_000,
                });
            })
        });
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    assert_eq!(reward.tokens.len(), 5);
    assert_ne!(ctx.payer.pubkey(), reward.creator);
    ctx.warp_to_timestamp(reward.deadline as i64 + 1);
    let token_accounts = refund_token_accounts(&mut ctx, &reward.tokens, &vault, &reward.creator);
    let instruction = ctx.portal().refund_intent_instruction(
        destination,
        reward.clone(),
        vault,
        route_hash,
        proof,
        withdrawn_marker,
        reward.creator,
        portal::instructions::RefundKind::Expired,
        Some(reward.prover),
        token_accounts,
        vec![],
    );
    let payer = ctx.payer.pubkey();
    let legacy = Transaction::new(
        &[&ctx.payer],
        Message::new(std::slice::from_ref(&instruction), Some(&payer)),
        ctx.latest_blockhash(),
    );
    assert!(serialized_len(&legacy) > PACKET_DATA_SIZE);
    let slot = ctx.get_sysvar::<SlotHashes>().first().unwrap().0;
    let (create, table) = create_lookup_table(payer, payer, slot);
    let addresses = instruction
        .accounts
        .iter()
        .filter(|account| !account.is_signer)
        .map(|account| account.pubkey)
        .collect::<Vec<_>>();
    let extend = extend_lookup_table(table, payer, Some(payer), addresses.clone());
    let transaction = Transaction::new(
        &[&ctx.payer],
        Message::new(&[create, extend], Some(&payer)),
        ctx.latest_blockhash(),
    );
    ctx.send_transaction(transaction).unwrap();
    let message = v0::Message::try_compile(
        &payer,
        &[instruction],
        &[AddressLookupTableAccount {
            key: table,
            addresses,
        }],
        ctx.latest_blockhash(),
    )
    .unwrap();
    let transaction =
        VersionedTransaction::try_new(VersionedMessage::V0(message), &[&ctx.payer]).unwrap();
    assert!(bincode::serialize(&transaction).unwrap().len() <= PACKET_DATA_SIZE);
    let result = ctx.send_transaction(transaction);

    assert!(result.is_ok_and(common::contains_event(IntentRefunded::new(
        intent_hash,
        reward.creator,
    ))));
    assert_eq!(ctx.balance(&reward.creator), reward.native_amount);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(
            ctx.token_balance_ata(&token.token, &reward.creator),
            token.amount
        );
    });
}
