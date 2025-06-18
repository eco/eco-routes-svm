use std::iter;

use anchor_lang::prelude::AccountMeta;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use portal::events::IntentFunded;
use portal::instructions::PortalError;
use portal::state;
use portal::types::{intent_hash, TokenAmount};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn fund_intent_native_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let vault_balance = ctx.balance(&vault_pda);
    let fund_amount = intent.reward.native_amount;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, fund_amount).unwrap();

    let result = ctx.fund_intent(&intent, vault_pda, route_hash, true, vec![]);
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        false,
    ))));
    assert_eq!(ctx.balance(&vault_pda) - vault_balance, fund_amount);
    assert_eq!(ctx.balance(&funder), 0);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
}

#[test]
fn fund_intent_tokens_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();

    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&Pubkey::new_from_array(token.token), &funder, token.amount);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        true,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        false,
    ))));
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        assert_eq!(ctx.token_balance_ata(&mint, &funder), 0);
        assert_eq!(ctx.token_balance_ata(&mint, &vault_pda), token.amount);
    });
}

#[test]
fn fund_intent_tokens_2022_success() {
    let mut ctx = common::Context::new_with_token_2022();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();

    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&Pubkey::new_from_array(token.token), &funder, token.amount);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        true,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        false,
    ))));
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        assert_eq!(ctx.token_balance_ata(&mint, &funder), 0);
        assert_eq!(ctx.token_balance_ata(&mint, &vault_pda), token.amount);
    });
}

#[test]
fn fund_intent_native_and_token_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let vault_balance = ctx.balance(&vault_pda);
    let fund_amount_native = intent.reward.native_amount;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();

    ctx.airdrop(&funder, fund_amount_native).unwrap();
    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&Pubkey::new_from_array(token.token), &funder, token.amount);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        false,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        true,
    ))));
    assert_eq!(ctx.balance(&vault_pda) - vault_balance, fund_amount_native);
    assert_eq!(ctx.balance(&funder), 0);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        assert_eq!(ctx.token_balance_ata(&mint, &funder), 0);
        assert_eq!(ctx.token_balance_ata(&mint, &vault_pda), token.amount);
    });
}

#[test]
fn fund_intent_native_and_token_with_zero_amounts_success() {
    let mut ctx = common::Context::default();
    let mut intent = ctx.rand_intent();
    intent.reward.native_amount = 0;
    intent.reward.tokens.iter_mut().for_each(|token| {
        token.amount = 0;
    });
    let route_hash = random();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();

    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&Pubkey::new_from_array(token.token), &funder, 0);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        false,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        true,
    ))));
    assert_eq!(ctx.balance(&vault_pda), 0);
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        assert_eq!(ctx.token_balance_ata(&mint, &vault_pda), 0);
    });
}

#[test]
fn fund_intent_duplicate_tokens_success() {
    let mut ctx = common::Context::default();
    let mut intent = ctx.rand_intent();
    let token_1 = Pubkey::new_unique();
    let token_2 = Pubkey::new_unique();
    intent.reward.tokens = vec![
        TokenAmount {
            token: *token_1.as_array(),
            amount: 1_000_000,
        },
        TokenAmount {
            token: *token_2.as_array(),
            amount: 1_000_000,
        },
        TokenAmount {
            token: *token_1.as_array(),
            amount: 1_000_000,
        },
    ];
    ctx.set_mint_account(&token_1);
    ctx.set_mint_account(&token_2);
    let route_hash = random();
    let payer_balance = ctx.balance(&ctx.payer.pubkey());
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();

    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&Pubkey::new_from_array(token.token), &funder, token.amount);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        true,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        false,
    ))));
    assert!(ctx.balance(&ctx.payer.pubkey()) < payer_balance);
    assert_eq!(ctx.token_balance_ata(&token_1, &funder), 0);
    assert_eq!(ctx.token_balance_ata(&token_2, &funder), 0);
    assert_eq!(ctx.token_balance_ata(&token_1, &vault_pda), 2_000_000);
    assert_eq!(ctx.token_balance_ata(&token_2, &vault_pda), 1_000_000);
}

#[test]
fn fund_intent_with_existing_vault_funds_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();
    let fund_amount_native = intent.reward.native_amount - intent.reward.native_amount / 3;

    ctx.airdrop(&vault_pda, intent.reward.native_amount / 3)
        .unwrap();
    ctx.airdrop(&funder, fund_amount_native).unwrap();

    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        ctx.airdrop_token_ata(&mint, &vault_pda, token.amount / 2);
        ctx.airdrop_token_ata(&mint, &funder, token.amount - token.amount / 2);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        true,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        true,
    ))));
    assert_eq!(ctx.balance(&vault_pda), intent.reward.native_amount);
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        assert_eq!(ctx.token_balance_ata(&mint, &vault_pda), token.amount);
        assert_eq!(ctx.token_balance_ata(&mint, &funder), 0);
    });
}

#[test]
fn fund_intent_already_funded_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let fund_amount_native = intent.reward.native_amount;

    ctx.airdrop(&vault_pda, fund_amount_native).unwrap();
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        ctx.airdrop_token_ata(&mint, &vault_pda, token.amount);
    });

    let result = ctx.fund_intent(&intent, vault_pda, route_hash, true, vec![]);
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        false,
    ))));
    assert_eq!(ctx.balance(&funder), 0);
    assert_eq!(ctx.balance(&vault_pda), fund_amount_native);
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        assert_eq!(ctx.token_balance_ata(&mint, &vault_pda), token.amount);
        assert_eq!(ctx.token_balance_ata(&mint, &funder), 0);
    });
}

#[test]
fn fund_intent_insufficient_funds_partial_allowed_success() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();
    let partial_native_amount = intent.reward.native_amount / 2;

    ctx.airdrop(&funder, partial_native_amount).unwrap();
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        ctx.airdrop_token_ata(&mint, &funder, token.amount / 2);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        true,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_ok_and(common::contains_event(IntentFunded::new(
        intent_hash(intent.route_chain, route_hash, &intent.reward),
        ctx.funder.pubkey(),
        false,
    ))));
    assert_eq!(ctx.balance(&funder), 0);
    assert_eq!(ctx.balance(&vault_pda), partial_native_amount);
    intent.reward.tokens.iter().for_each(|token| {
        let mint = Pubkey::new_from_array(token.token);

        assert_eq!(ctx.token_balance_ata(&mint, &funder), 0);
        assert_eq!(ctx.token_balance_ata(&mint, &vault_pda), token.amount / 2);
    });
}

#[test]
fn fund_intent_invalid_vault_fails() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let fund_amount = intent.reward.native_amount;
    let funder = ctx.funder.pubkey();

    ctx.airdrop(&funder, fund_amount).unwrap();

    let result = ctx.fund_intent(&intent, Pubkey::new_unique(), route_hash, true, vec![]);
    assert!(result.is_err_and(common::is_portal_error(PortalError::InvalidVault)));
}

#[test]
fn fund_intent_insufficient_native_funds_fails() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();

    let insufficient_amount = intent.reward.native_amount / 2;
    ctx.airdrop(&funder, insufficient_amount).unwrap();

    let result = ctx.fund_intent(&intent, vault_pda, route_hash, false, vec![]);
    assert!(result.is_err_and(common::is_portal_error(PortalError::InsufficientFunds)));
}

#[test]
fn fund_intent_insufficient_token_funds_fails() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();
    let fund_amount_native = intent.reward.native_amount;

    ctx.airdrop(&funder, fund_amount_native).unwrap();
    intent.reward.tokens.iter().for_each(|token| {
        let insufficient_token_amount = token.amount / 2;
        ctx.airdrop_token_ata(
            &Pubkey::new_from_array(token.token),
            &funder,
            insufficient_token_amount,
        );
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        false,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let vault_ata =
                get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_err_and(common::is_portal_error(PortalError::InsufficientFunds)));
}

#[test]
fn fund_intent_invalid_vault_ata_fails() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();

    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&Pubkey::new_from_array(token.token), &funder, token.amount);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        true,
        intent.reward.tokens.iter().flat_map(|token| {
            let mint = Pubkey::new_from_array(token.token);
            let funder_token =
                get_associated_token_address_with_program_id(&funder, &mint, token_program);
            let wrong_vault_ata = Pubkey::new_unique();

            vec![
                AccountMeta::new(funder_token, false),
                AccountMeta::new(wrong_vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ]
        }),
    );
    assert!(result.is_err_and(common::is_portal_error(PortalError::InvalidVaultAta)));
}

#[test]
fn fund_intent_invalid_mint_fails() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();
    let wrong_mint = Pubkey::new_unique();
    let funder_token =
        get_associated_token_address_with_program_id(&funder, &wrong_mint, token_program);
    let vault_ata =
        get_associated_token_address_with_program_id(&vault_pda, &wrong_mint, token_program);

    ctx.set_mint_account(&wrong_mint);
    ctx.airdrop_token_ata(&wrong_mint, &funder, 1000);

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        true,
        vec![
            AccountMeta::new(funder_token, false),
            AccountMeta::new(vault_ata, false),
            AccountMeta::new_readonly(wrong_mint, false),
        ],
    );
    assert!(result.is_err_and(common::is_portal_error(PortalError::InvalidMint)));
}

#[test]
fn fund_intent_invalid_token_transfer_accounts_fails() {
    let mut ctx = common::Context::default();
    let intent = ctx.rand_intent();
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;
    let funder = ctx.funder.pubkey();
    let token_program = &ctx.token_program.clone();

    intent.reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&Pubkey::new_from_array(token.token), &funder, token.amount);
    });

    let result = ctx.fund_intent(
        &intent,
        vault_pda,
        route_hash,
        true,
        intent
            .reward
            .tokens
            .iter()
            .flat_map(|token| {
                let mint = Pubkey::new_from_array(token.token);
                let funder_token =
                    get_associated_token_address_with_program_id(&funder, &mint, token_program);
                let vault_ata =
                    get_associated_token_address_with_program_id(&vault_pda, &mint, token_program);

                vec![
                    AccountMeta::new(funder_token, false),
                    AccountMeta::new(vault_ata, false),
                    AccountMeta::new_readonly(mint, false),
                ]
            })
            .chain(iter::once(AccountMeta::new(Pubkey::new_unique(), false))),
    );
    assert!(result.is_err_and(common::is_portal_error(
        PortalError::InvalidTokenTransferAccounts
    )));
}

#[test]
fn fund_intent_token_amount_overflow_fails() {
    let mut ctx = common::Context::default();
    let token_mint = Pubkey::new_unique();
    let mut intent = ctx.rand_intent();
    intent.reward.tokens = vec![
        TokenAmount {
            token: *token_mint.as_array(),
            amount: u64::MAX - 1000,
        },
        TokenAmount {
            token: *token_mint.as_array(),
            amount: 2000,
        },
    ];
    let route_hash = random();
    let vault_pda = state::Vault::pda(intent.route_chain, route_hash, &intent.reward).0;

    let result = ctx.fund_intent(&intent, vault_pda, route_hash, true, vec![]);
    assert!(result.is_err_and(common::is_portal_error(PortalError::RewardAmountOverflow)));
}
