use anchor_lang::error::ErrorCode;
use anchor_lang::prelude::AccountMeta;
use anchor_lang::AnchorSerialize;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::spl_token;
use eco_svm_std::prover::Proof;
use eco_svm_std::CHAIN_ID;
use flash_fulfiller::events::FlashFulfilled;
use flash_fulfiller::instructions::{FlashFulfillIntent, FlashFulfillerError};
use flash_fulfiller::state::{flash_vault_pda, FlashFulfillIntentAccount};
use portal::state::{executor_pda, vault_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{
    intent_hash, Call, Calldata, CalldataWithAccounts, Reward, Route, TokenAmount,
};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

fn setup() -> (common::Context, Route, Reward, Pubkey) {
    setup_with_ctx(common::Context::default())
}

fn setup_with_ctx(mut ctx: common::Context) -> (common::Context, Route, Reward, Pubkey) {
    let (_, mut route, mut reward) = ctx.rand_intent();

    reward.prover = local_prover::ID;
    route.calls.clear();
    route.native_amount = reward.native_amount / 2;
    route.tokens = reward
        .tokens
        .iter()
        .map(|reward_token| TokenAmount {
            token: reward_token.token,
            amount: reward_token.amount / 2,
        })
        .collect();

    let route_hash = route.hash();
    let intent_hash_value = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = vault_pda(&intent_hash_value).0;

    let funder = ctx.funder.pubkey();
    ctx.airdrop(&funder, reward.native_amount).unwrap();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &funder, token.amount);
    });

    let token_program = ctx.token_program;
    let fund_accounts: Vec<AccountMeta> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            vec![
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &funder,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &vault,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    ctx.portal()
        .fund_intent(
            CHAIN_ID,
            reward.clone(),
            vault,
            route_hash,
            false,
            fund_accounts,
        )
        .unwrap();

    (ctx, route, reward, vault)
}

fn claimant_atas(ctx: &common::Context, reward: &Reward, claimant: Pubkey) -> Vec<AccountMeta> {
    let token_program = ctx.token_program;
    reward
        .tokens
        .iter()
        .map(|token| {
            AccountMeta::new(
                get_associated_token_address_with_program_id(
                    &claimant,
                    &token.token,
                    &token_program,
                ),
                false,
            )
        })
        .collect()
}

#[test]
fn flash_fulfill_should_succeed() {
    let (mut ctx, route, reward, _vault) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_ok_and(common::contains_cpi_event(FlashFulfilled {
        intent_hash: intent_hash_value,
        claimant,
        native_fee: reward.native_amount - route.native_amount,
    })));

    reward
        .tokens
        .iter()
        .zip(route.tokens.iter())
        .for_each(|(reward_token, route_token)| {
            assert_eq!(
                ctx.token_balance_ata(&reward_token.token, &claimant),
                reward_token.amount - route_token.amount,
            );
        });
    assert_eq!(
        ctx.balance(&claimant),
        reward.native_amount - route.native_amount,
    );
    assert_eq!(ctx.balance(&flash_vault_pda().0), 0);
    assert!(ctx
        .account::<WithdrawnMarker>(&WithdrawnMarker::pda(&intent_hash_value).0)
        .is_some());
    assert!(ctx
        .account::<FulfillMarker>(&FulfillMarker::pda(&intent_hash_value).0)
        .is_some());
    assert!(ctx
        .account::<local_prover::state::ProofAccount>(
            &Proof::pda(&intent_hash_value, &local_prover::ID).0,
        )
        .is_none());
}

#[test]
fn flash_fulfill_default_claimant_fail() {
    let (mut ctx, route, reward, _) = setup();
    let claimant = Pubkey::default();

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidClaimant)));
}

#[test]
fn flash_fulfill_wrong_claimant_ata_fail() {
    let (mut ctx, route, reward, _) = setup();
    let claimant = Pubkey::new_unique();
    let impostor = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &impostor, 0);
    });

    let impostor_atas = claimant_atas(&ctx, &reward, impostor);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        &route,
        &reward,
        claimant,
        impostor_atas,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidClaimantToken)));
}

#[test]
fn flash_fulfill_from_buffer_should_succeed() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value, &writer.pubkey()).0;
    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route.clone(), reward.clone())
        .unwrap();

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_ok_and(common::contains_cpi_event(FlashFulfilled {
        intent_hash: intent_hash_value,
        claimant,
        native_fee: reward.native_amount - route.native_amount,
    })));
    assert!(ctx.account::<FlashFulfillIntentAccount>(&buffer).is_none());
}

#[test]
fn flash_fulfill_intent_hash_missing_buffer_fail() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(ErrorCode::AccountNotInitialized)));
}

#[test]
fn flash_fulfill_with_calls_should_succeed() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.native_amount = reward.native_amount / 2;
    route.tokens = reward
        .tokens
        .iter()
        .map(|reward_token| TokenAmount {
            token: reward_token.token,
            amount: reward_token.amount / 2,
        })
        .collect();

    let token_program = ctx.token_program;
    let executor = executor_pda().0;
    let recipient = Pubkey::new_unique();

    let (calldatas, call_account_metas): (Vec<_>, Vec<_>) = route
        .tokens
        .iter()
        .map(|route_token| {
            let executor_ata = get_associated_token_address_with_program_id(
                &executor,
                &route_token.token,
                &token_program,
            );
            let recipient_ata = get_associated_token_address_with_program_id(
                &recipient,
                &route_token.token,
                &token_program,
            );
            let calldata = Calldata {
                data: spl_token::instruction::transfer_checked(
                    &token_program,
                    &executor_ata,
                    &route_token.token,
                    &recipient_ata,
                    &executor,
                    &[],
                    route_token.amount,
                    6,
                )
                .unwrap()
                .data,
                account_count: 4,
            };
            let call_accounts = vec![
                AccountMeta::new(executor_ata, false),
                AccountMeta::new_readonly(route_token.token, false),
                AccountMeta::new(recipient_ata, false),
                AccountMeta::new(executor, false),
            ];

            (calldata, call_accounts)
        })
        .unzip();

    route.calls = calldatas
        .iter()
        .zip(call_account_metas.iter())
        .map(|(calldata, call_accounts)| Call {
            target: token_program.to_bytes().into(),
            data: CalldataWithAccounts::new(calldata.clone(), call_accounts.clone())
                .unwrap()
                .try_to_vec()
                .unwrap(),
        })
        .collect();

    let route_hash = route.hash();
    let intent_hash_value = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = vault_pda(&intent_hash_value).0;

    let funder = ctx.funder.pubkey();
    ctx.airdrop(&funder, reward.native_amount).unwrap();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &funder, token.amount);
    });
    route.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &recipient, 0);
    });

    let fund_accounts: Vec<AccountMeta> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            vec![
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &funder,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &vault,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    ctx.portal()
        .fund_intent(
            CHAIN_ID,
            reward.clone(),
            vault,
            route_hash,
            false,
            fund_accounts,
        )
        .unwrap();

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let flattened_call_accounts: Vec<AccountMeta> =
        call_account_metas.into_iter().flatten().collect();
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        flattened_call_accounts,
    );

    assert!(result.is_ok_and(common::contains_cpi_event(FlashFulfilled {
        intent_hash: intent_hash_value,
        claimant,
        native_fee: reward.native_amount - route.native_amount,
    })));

    route.tokens.iter().for_each(|route_token| {
        assert_eq!(
            ctx.token_balance_ata(&route_token.token, &recipient),
            route_token.amount,
        );
    });
    reward
        .tokens
        .iter()
        .zip(route.tokens.iter())
        .for_each(|(reward_token, route_token)| {
            assert_eq!(
                ctx.token_balance_ata(&reward_token.token, &claimant),
                reward_token.amount - route_token.amount,
            );
        });
    assert!(ctx
        .account::<WithdrawnMarker>(&WithdrawnMarker::pda(&intent_hash_value).0)
        .is_some());
    assert!(ctx
        .account::<FulfillMarker>(&FulfillMarker::pda(&intent_hash_value).0)
        .is_some());
}

#[test]
fn flash_fulfill_token_2022_should_succeed() {
    let (mut ctx, route, reward, _) = setup_with_ctx(common::Context::new_with_token_2022());
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_ok_and(common::contains_cpi_event(FlashFulfilled {
        intent_hash: intent_hash_value,
        claimant,
        native_fee: reward.native_amount - route.native_amount,
    })));

    reward
        .tokens
        .iter()
        .zip(route.tokens.iter())
        .for_each(|(reward_token, route_token)| {
            assert_eq!(
                ctx.token_balance_ata(&reward_token.token, &claimant),
                reward_token.amount - route_token.amount,
            );
        });
    assert_eq!(
        ctx.balance(&claimant),
        reward.native_amount - route.native_amount,
    );
    assert_eq!(ctx.balance(&flash_vault_pda().0), 0);
}

#[test]
fn flash_fulfill_zero_leftover_should_succeed() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();
    route.native_amount = reward.native_amount;
    route.tokens = reward.tokens.clone();

    let route_hash = route.hash();
    let intent_hash_value = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = vault_pda(&intent_hash_value).0;

    let funder = ctx.funder.pubkey();
    ctx.airdrop(&funder, reward.native_amount).unwrap();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &funder, token.amount);
    });

    let token_program = ctx.token_program;
    let fund_accounts: Vec<AccountMeta> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            vec![
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &funder,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &vault,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    ctx.portal()
        .fund_intent(
            CHAIN_ID,
            reward.clone(),
            vault,
            route_hash,
            false,
            fund_accounts,
        )
        .unwrap();

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_ok_and(common::contains_cpi_event(FlashFulfilled {
        intent_hash: intent_hash_value,
        claimant,
        native_fee: 0,
    })));

    reward.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &claimant), 0);
    });
    assert_eq!(ctx.balance(&claimant), 0);
    assert_eq!(ctx.balance(&flash_vault_pda().0), 0);
    assert!(ctx
        .account::<WithdrawnMarker>(&WithdrawnMarker::pda(&intent_hash_value).0)
        .is_some());
    assert!(ctx
        .account::<FulfillMarker>(&FulfillMarker::pda(&intent_hash_value).0)
        .is_some());
}
