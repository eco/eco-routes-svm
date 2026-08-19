use std::iter;

use anchor_lang::prelude::AccountMeta;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::Proof;
use eco_svm_std::Bytes32;
use hyper_prover::state::pda_payer_pda;
use portal::events::IntentWithdrawn;
use portal::state::{self, proof_closer_pda};
use portal::types::{intent_hash, Reward, Route, TokenAmount};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

pub mod common;

fn setup(is_token_2022: bool) -> (common::Context, (u64, Route, Reward), Bytes32) {
    let mut ctx = if is_token_2022 {
        common::Context::new_with_token_2022()
    } else {
        common::Context::default()
    };
    let intent = ctx.rand_intent();
    let (destination, _route, reward) = &intent;
    let route_hash = random::<[u8; 32]>().into();
    let funder = ctx.funder.pubkey();
    let vault_pda = state::vault_pda(&intent_hash(*destination, &route_hash, &reward.hash())).0;
    let token_program = &ctx.token_program.clone();

    ctx.airdrop(&funder, reward.native_amount).unwrap();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &funder, token.amount);
    });

    ctx.portal()
        .fund_intent(
            *destination,
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

    (ctx, intent, route_hash)
}

#[test]
fn withdraw_intent_native_and_token_success() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
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

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    assert_eq!(ctx.balance(&claimant), reward.native_amount);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &claimant), token.amount);
    });
    assert!(ctx.get_account(&proof).is_none());
}

#[test]
fn withdraw_intent_native_and_token_2022_success() {
    let (mut ctx, intent, route_hash) = setup(true);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
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

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    assert_eq!(ctx.balance(&claimant), reward.native_amount);
    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &claimant), token.amount);
    });
    assert!(ctx.get_account(&proof).is_none());
}

#[test]
fn withdraw_intent_over_funded_native_sub_rent_dust_success() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    // address-poisoning dust below the rent-exempt floor: draining only
    // reward.native_amount would leave a sub-rent residual and fail the tx
    let dust = 1_000;
    ctx.airdrop(&vault, dust).unwrap();
    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
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

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    assert_eq!(ctx.balance(&claimant), reward.native_amount + dust);
}

#[test]
fn withdraw_intent_invalid_vault_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let wrong_vault = Pubkey::new_unique();
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);

    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        wrong_vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let first_token = reward.tokens.first().unwrap();
    let claimant_token =
        get_associated_token_address_with_program_id(&claimant, &first_token.token, token_program);
    let vault_ata =
        get_associated_token_address_with_program_id(&vault, &first_token.token, token_program);
    let token_accounts: Vec<_> = reward
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

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let wrong_proof = Pubkey::new_unique();
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        wrong_proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program;

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
    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let wrong_claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);

    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        wrong_claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentNotFulfilled
    )));
}

#[test]
fn withdraw_intent_wrong_destination_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let wrong_destination = random();
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(
        proof,
        Proof::new(wrong_destination, claimant),
        hyper_prover::ID,
    );

    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
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

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
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
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let wrong_owner = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
        ctx.airdrop_token_ata(&token.token, &wrong_owner, 0);
    });

    let token_accounts: Vec<_> = reward
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

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidClaimantToken
    )));
}

/// The recovery route. Pinning the destination to the derived ATA would be
/// terminal if that ATA cannot receive — a mint freeze authority, or a
/// token-2022 `DefaultAccountState::Frozen` mint — because the whole withdraw
/// reverts, the `Proof` survives, and `refund` refuses for as long as it does.
/// The claimant, and only the claimant, may direct the payout elsewhere.
#[test]
fn withdraw_intent_claimant_signed_non_ata_destination_success() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant_keypair = Keypair::new();
    let claimant = claimant_keypair.pubkey();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);

    let destinations: Vec<Pubkey> = reward
        .tokens
        .iter()
        .map(|token| {
            let claimant_token = Pubkey::new_unique();
            ctx.set_token_account(claimant_token, &token.token, &claimant);
            claimant_token
        })
        .collect();
    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .zip(&destinations)
        .flat_map(|(token, claimant_token)| {
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(*claimant_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent_with_signers(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
        vec![&claimant_keypair],
    );
    assert!(
        result.is_ok_and(common::contains_event(IntentWithdrawn::new(
            intent_hash,
            claimant,
        )))
    );
    reward
        .tokens
        .iter()
        .zip(&destinations)
        .for_each(|(token, claimant_token)| {
            assert_eq!(ctx.token_balance(claimant_token), token.amount);
            assert_eq!(ctx.token_balance_ata(&token.token, &vault), 0);
        });
}

#[test]
fn withdraw_intent_non_ata_claimant_token_2022_fail() {
    let (mut ctx, intent, route_hash) = setup(true);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);

    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            // the claimant's ATA derived with the LEGACY program id, while the mint
            // is token-2022 — a real claimant-owned account at an address only a
            // wrong-program derivation would produce, so this fails only if
            // `claimant_ata` is derived with the mint's own program
            let claimant_token = get_associated_token_address_with_program_id(
                &claimant,
                &token.token,
                &anchor_spl::token::ID,
            );
            ctx.set_token_account(claimant_token, &token.token, &claimant);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(claimant_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::ClaimantSignatureRequired
    )));
}

#[test]
fn withdraw_intent_non_ata_claimant_token_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);

    let token_accounts: Vec<_> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            let claimant_token = Pubkey::new_unique();
            ctx.set_token_account(claimant_token, &token.token, &claimant);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, &token.token, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(claimant_token, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::ClaimantSignatureRequired
    )));
}

#[test]
fn withdraw_intent_already_withdrawn_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let token_program = &ctx.token_program.clone();

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
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

    let (destination, _route, reward) = &intent;
    ctx.portal()
        .withdraw_intent(
            *destination,
            reward.clone(),
            vault,
            route_hash,
            claimant,
            proof,
            withdrawn_marker,
            proof_closer_pda(&reward.prover).0,
            token_accounts.clone(),
            iter::once(AccountMeta::new(pda_payer_pda().0, false)),
        )
        .unwrap();

    let (destination, _route, reward) = &intent;
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentNotFulfilled
    )));
}

/// The binding this fix introduces: a **well-formed** closer scoped to a
/// different prover must be rejected. The random-key case above was rejected by
/// the pre-fix code just as readily, so it does not exercise the scoping.
#[test]
fn withdraw_intent_other_provers_proof_closer_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);

    // `reward.prover` is hyper-prover, so local-prover's closer is the wrong scope
    assert_eq!(reward.prover, hyper_prover::ID);
    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&local_prover::ID).0,
        vec![],
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidProofCloser
    )));
}

#[test]
fn withdraw_intent_invalid_proof_closer_fail() {
    let (mut ctx, intent, route_hash) = setup(false);
    let (destination, _route, reward) = &intent;
    let intent_hash = intent_hash(*destination, &route_hash, &reward.hash());
    let claimant = Pubkey::new_unique();
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;

    ctx.set_proof(proof, Proof::new(*destination, claimant), hyper_prover::ID);

    let result = ctx.portal().withdraw_intent(
        *destination,
        reward.clone(),
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

// `Reward.tokens` may carry several entries for the same mint: the account layout
/// Duplicate reward mints aggregate: the account layout is one triple per unique
/// mint, and each mint pays out its aggregated total.
///
/// `vault_surplus` is added to the duplicate mint's vault ATA beyond the reward
/// target — anyone can donate to a deterministic ATA address, and the payout must
/// stay capped at what the reward promises.
fn duplicate_reward_mints_case(is_token_2022: bool, vault_surplus: u64, funded: u64) -> (u64, u64) {
    let mut ctx = if is_token_2022 {
        common::Context::new_with_token_2022()
    } else {
        common::Context::default()
    };
    let (destination, _route, mut reward) = ctx.rand_intent();
    let duplicate_mint = reward.tokens[0].token;
    let other_mint = reward.tokens[1].token;
    reward.tokens = vec![
        TokenAmount {
            token: duplicate_mint,
            amount: 1_000_000,
        },
        TokenAmount {
            token: other_mint,
            amount: 500_000,
        },
        TokenAmount {
            token: duplicate_mint,
            amount: 2_000_000,
        },
    ];

    let route_hash = random::<[u8; 32]>().into();
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let claimant = Pubkey::new_unique();
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();

    ctx.airdrop(&funder, reward.native_amount).unwrap();
    ctx.airdrop_token_ata(&duplicate_mint, &funder, funded);
    ctx.airdrop_token_ata(&other_mint, &funder, 500_000);

    let reward_token_amounts = reward.token_amounts().unwrap();
    ctx.portal()
        .fund_intent(
            destination,
            reward.clone(),
            vault,
            route_hash,
            true,
            reward_token_amounts.keys().flat_map(|mint| {
                let funder_token =
                    get_associated_token_address_with_program_id(&funder, mint, token_program);
                let vault_ata =
                    get_associated_token_address_with_program_id(&vault, mint, token_program);

                vec![
                    AccountMeta::new(funder_token, false),
                    AccountMeta::new(vault_ata, false),
                    AccountMeta::new_readonly(*mint, false),
                ]
            }),
        )
        .unwrap();

    // a third party donating to the vault ATA must not raise the payout
    if vault_surplus > 0 {
        ctx.airdrop_token_ata(&duplicate_mint, &vault, vault_surplus);
    }

    ctx.set_proof(proof, Proof::new(destination, claimant), hyper_prover::ID);
    reward_token_amounts.keys().for_each(|mint| {
        ctx.airdrop_token_ata(mint, &claimant, 0);
    });

    // supplied in reverse: the mint check compares sets, so any permutation of
    // the triples is valid — only the count has to match the unique-mint count
    let token_accounts: Vec<_> = reward_token_amounts
        .keys()
        .rev()
        .flat_map(|mint| {
            let claimant_token =
                get_associated_token_address_with_program_id(&claimant, mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault, mint, token_program);

            vec![
                AccountMeta::new(vault_ata, false),
                AccountMeta::new(claimant_token, false),
                AccountMeta::new_readonly(*mint, false),
            ]
        })
        .collect();

    let vault_native = ctx.balance(&vault);
    let result = ctx.portal().withdraw_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    assert!(
        result.is_ok_and(common::contains_event(IntentWithdrawn::new(
            intent_hash,
            claimant,
        )))
    );
    assert_eq!(ctx.token_balance_ata(&other_mint, &claimant), 500_000);

    // the split index decides the tail handed to `close_proof`, so an off-by-a-triple
    // split would corrupt it — assert the native leg and the closure too
    assert!(vault_native > 0);
    assert_eq!(ctx.balance(&claimant), reward.native_amount);
    assert_eq!(ctx.balance(&vault), 0);
    assert!(ctx.get_account(&proof).is_none());

    (
        ctx.token_balance_ata(&duplicate_mint, &claimant),
        ctx.token_balance_ata(&duplicate_mint, &vault),
    )
}

/// A legacy client sending one triple per **raw** reward entry must fail closed
/// rather than mis-slice: the surplus triple falls into the tail that portal
/// forwards to `close_proof`, where it is rejected positionally.
#[test]
fn withdraw_intent_duplicate_reward_mints_raw_layout_fail() {
    let mut ctx = common::Context::default();
    let (destination, _route, mut reward) = ctx.rand_intent();
    let duplicate_mint = reward.tokens[0].token;
    let other_mint = reward.tokens[1].token;
    reward.tokens = vec![
        TokenAmount {
            token: duplicate_mint,
            amount: 1_000_000,
        },
        TokenAmount {
            token: other_mint,
            amount: 500_000,
        },
        TokenAmount {
            token: duplicate_mint,
            amount: 2_000_000,
        },
    ];

    let route_hash = random::<[u8; 32]>().into();
    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    let vault = state::vault_pda(&intent_hash).0;
    let proof = Proof::pda(&intent_hash, &reward.prover).0;
    let withdrawn_marker = state::WithdrawnMarker::pda(&intent_hash).0;
    let claimant = Pubkey::new_unique();
    let token_program = &ctx.token_program.clone();

    // fund for real, so the failure is about the layout and not an uninitialized
    // vault ATA
    let funder = ctx.funder.pubkey();
    ctx.airdrop(&funder, reward.native_amount).unwrap();
    ctx.airdrop_token_ata(&duplicate_mint, &funder, 3_000_000);
    ctx.airdrop_token_ata(&other_mint, &funder, 500_000);
    let reward_token_amounts = reward.token_amounts().unwrap();
    ctx.portal()
        .fund_intent(
            destination,
            reward.clone(),
            vault,
            route_hash,
            false,
            reward_token_amounts.keys().flat_map(|mint| {
                let funder_token =
                    get_associated_token_address_with_program_id(&funder, mint, token_program);
                let vault_ata =
                    get_associated_token_address_with_program_id(&vault, mint, token_program);

                vec![
                    AccountMeta::new(funder_token, false),
                    AccountMeta::new(vault_ata, false),
                    AccountMeta::new_readonly(*mint, false),
                ]
            }),
        )
        .unwrap();

    ctx.set_proof(proof, Proof::new(destination, claimant), hyper_prover::ID);
    reward_token_amounts.keys().for_each(|mint| {
        ctx.airdrop_token_ata(mint, &claimant, 0);
    });

    // one triple per RAW entry — three, where the layout wants two
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

    let result = ctx.portal().withdraw_intent(
        destination,
        reward.clone(),
        vault,
        route_hash,
        claimant,
        proof,
        withdrawn_marker,
        proof_closer_pda(&reward.prover).0,
        token_accounts,
        iter::once(AccountMeta::new(pda_payer_pda().0, false)),
    );
    // the surplus triple lands in the tail portal forwards to `close_proof`,
    // where hyper-prover's address-pinned `pda_payer` rejects it
    assert!(result.is_err_and(common::is_error(
        hyper_prover::instructions::HyperProverError::InvalidPdaPayer
    )));
}

#[test]
fn withdraw_intent_duplicate_reward_mints_success() {
    let (paid, vault_left) = duplicate_reward_mints_case(false, 0, 3_000_000);

    assert_eq!(paid, 3_000_000);
    assert_eq!(vault_left, 0);
}

#[test]
fn withdraw_intent_duplicate_reward_mints_token_2022_success() {
    let (paid, vault_left) = duplicate_reward_mints_case(true, 0, 3_000_000);

    assert_eq!(paid, 3_000_000);
    assert_eq!(vault_left, 0);
}

/// The payout is capped at the aggregated reward, not the vault balance.
#[test]
fn withdraw_intent_duplicate_reward_mints_over_funded_success() {
    let (paid, vault_left) = duplicate_reward_mints_case(false, 750_000, 3_000_000);

    assert_eq!(paid, 3_000_000);
    assert_eq!(vault_left, 750_000);
}

/// `min(reward, vault)` the other way: an under-funded vault pays what it holds.
#[test]
fn withdraw_intent_duplicate_reward_mints_partially_funded_success() {
    let (paid, vault_left) = duplicate_reward_mints_case(false, 0, 1_800_000);

    assert_eq!(paid, 1_800_000);
    assert_eq!(vault_left, 0);
}
