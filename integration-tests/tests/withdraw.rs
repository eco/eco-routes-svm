use std::iter;

use anchor_lang::prelude::AccountMeta;
use anchor_lang::system_program;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::Proof;
use eco_svm_std::Bytes32;
use hyper_prover::state::pda_payer_pda;
use portal::events::IntentWithdrawn;
use portal::state::{self, proof_closer_pda};
use portal::types::{intent_hash, Intent};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

fn setup(is_token_2022: bool) -> (common::Context, Intent, Bytes32) {
    let mut ctx = if is_token_2022 {
        common::Context::new_with_token_2022()
    } else {
        common::Context::default()
    };
    let intent = ctx.rand_intent();
    let route_hash = random::<[u8; 32]>().into();
    let funder = ctx.funder.pubkey();
    let vault_pda = state::vault_pda(&intent_hash(
        intent.destination_chain,
        &route_hash,
        &intent.reward.hash(),
    ))
    .0;
    let token_program = &ctx.token_program.clone();

    ctx.airdrop(&funder, intent.reward.native_amount).unwrap();
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &funder, token.amount);
    });

    ctx.portal()
        .fund_intent(
            &intent,
            vault_pda,
            route_hash,
            false,
            intent.reward.tokens.iter().flat_map(|token| {
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

    (ctx, intent, route_hash)
}

#[test]
fn withdraw_intent_native_and_token_success() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let token_accounts: Vec<_> = intent
        .reward
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

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(
        result.is_ok_and(common::contains_event(IntentWithdrawn::new(
            intent_hash,
            claimant,
        )))
    );
    assert_eq!(ctx.balance(&vault), 0);
    assert_eq!(ctx.balance(&claimant), intent.reward.native_amount);
    intent.reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &claimant), token.amount);
    });
    let proof_account = ctx.get_account(&proof).unwrap();
    assert!(proof_account.data.is_empty());
    assert_eq!(proof_account.owner, system_program::ID);
    assert_eq!(proof_account.lamports, 0);
}

#[test]
fn withdraw_intent_native_and_token_2022_success() {
    let (mut ctx, intent, route_hash) = setup(true);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let token_accounts: Vec<_> = intent
        .reward
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

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(
        result.is_ok_and(common::contains_event(IntentWithdrawn::new(
            intent_hash,
            claimant,
        )))
    );
    assert_eq!(ctx.balance(&vault), 0);
    assert_eq!(ctx.balance(&claimant), intent.reward.native_amount);
    intent.reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &claimant), token.amount);
    });
    let proof_account = ctx.get_account(&proof).unwrap();
    assert!(proof_account.data.is_empty());
    assert_eq!(proof_account.owner, system_program::ID);
    assert_eq!(proof_account.lamports, 0);
}

#[test]
fn withdraw_intent_invalid_vault_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let wrong_vault = Pubkey::new_unique();
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );

    let result = ctx.portal().withdraw_intent(
        &intent,
        wrong_vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidVault
    )));
}

#[test]
fn withdraw_intent_duplicate_mint_accounts_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let first_token = intent.reward.tokens.first().unwrap();
    let claimant_token =
        get_associated_token_address_with_program_id(&claimant, &first_token.token, token_program);
    let vault_ata =
        get_associated_token_address_with_program_id(&vault, &first_token.token, token_program);
    let token_accounts: Vec<_> = intent
        .reward
        .tokens
        .iter()
        .flat_map(|_| {
            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(claimant_token, false),
                AccountMeta::new_readonly(first_token.token, false),
            ]
        })
        .collect();

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidMint
    )));
}

#[test]
fn withdraw_intent_invalid_proof_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let wrong_proof = Pubkey::new_unique();
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        wrong_proof,
        withdrawn_marker,
        proof_closer_pda().0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidProof
    )));
}

#[test]
fn withdraw_intent_not_fulfilled_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program;

    let token_accounts: Vec<_> = intent
        .reward
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
    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentNotFulfilled
    )));
}

#[test]
fn withdraw_intent_wrong_claimant_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let wrong_claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        wrong_claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentNotFulfilled
    )));
}

#[test]
fn withdraw_intent_wrong_destination_chain_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let wrong_destination_chain = random();
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(
        proof,
        Proof::new(wrong_destination_chain, claimant),
        hyper_prover::ID,
    );

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentNotFulfilled
    )));
}

#[test]
fn withdraw_intent_invalid_token_transfer_accounts() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidTokenTransferAccounts
    )));
}

#[test]
fn withdraw_intent_invalid_vault_ata_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let token_accounts: Vec<_> = intent
        .reward
        .tokens
        .iter()
        .flat_map(|token| {
            let claimant_token = get_associated_token_address_with_program_id(
                &claimant,
                &token.token,
                token_program,
            );
            let wrong_vault_ata = get_associated_token_address_with_program_id(
                &claimant, // Wrong! Should be vault
                &token.token,
                token_program,
            );

            vec![
                AccountMeta::new(wrong_vault_ata, false), // Wrong vault ATA
                AccountMeta::new(claimant_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidAta
    )));
}

#[test]
fn withdraw_intent_invalid_claimant_token_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let wrong_owner = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
        ctx.airdrop_token_ata(&token.token, &wrong_owner, 0);
    });

    let token_accounts: Vec<_> = intent
        .reward
        .tokens
        .iter()
        .flat_map(|token| {
            let wrong_claimant_token = get_associated_token_address_with_program_id(
                &wrong_owner,
                &token.token,
                token_program,
            );
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(wrong_claimant_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidClaimantToken
    )));
}

#[test]
fn withdraw_intent_already_withdrawn_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let token_accounts: Vec<_> = intent
        .reward
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
            &intent,
            vault,
            route_hash,
            claimant,
            proof,
            withdrawn_marker,
            proof_closer_pda().0,
            token_accounts.clone(),
            iter::once(AccountMeta::new(pda_payer_pda().0, false)),
        )
        .unwrap();

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda().0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentNotFulfilled
    )));
}

#[test]
fn withdraw_intent_invalid_proof_closer_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let intent_hash = intent_hash(intent.destination_chain, &route_hash, &intent.reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &intent.reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(
        proof,
        Proof::new(intent.destination_chain, claimant),
        hyper_prover::ID,
    );

    let result = ctx.portal().withdraw_intent(
        &intent,
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        Pubkey::new_unique(),
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidProofCloser
    )));
}
