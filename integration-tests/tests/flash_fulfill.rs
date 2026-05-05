use anchor_lang::error::ErrorCode;
use anchor_lang::prelude::AccountMeta;
use anchor_lang::{AnchorSerialize, Discriminator};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::spl_token;
use eco_svm_std::prover::Proof;
use eco_svm_std::{SerializableAccountMeta, CHAIN_ID};
use flash_fulfiller::events::FlashFulfilled;
use flash_fulfiller::instructions::{FlashFulfillIntent, FlashFulfillerError};
use flash_fulfiller::state::{flash_vault_pda, FlashFulfillIntentAccount};
use portal::state::{executor_pda, vault_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{
    intent_hash, Call, Calldata, CalldataWithAccounts, Reward, Route, TokenAmount,
};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
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
        None,
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
fn flash_fulfill_native_fee_reports_documented_delta_not_swept_amount() {
    let (mut ctx, route, reward, _vault) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let donation: u64 = 7777;
    ctx.airdrop(&flash_vault_pda().0, donation).unwrap();

    let documented_delta = reward.native_amount.saturating_sub(route.native_amount);

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        None,
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_ok_and(common::contains_cpi_event(FlashFulfilled {
        intent_hash: intent_hash_value,
        claimant,
        native_fee: documented_delta,
    })));
    assert_eq!(ctx.balance(&claimant), documented_delta + donation);
    assert_eq!(ctx.balance(&flash_vault_pda().0), 0);
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
        None,
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
        None,
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

    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;
    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer, buffer, route.clone(), reward.clone())
        .unwrap();

    let writer_balance_before = ctx.balance(&writer.pubkey());
    let buffer_rent = ctx.balance(&buffer);

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        Some(writer.pubkey()),
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
    assert_eq!(
        ctx.balance(&writer.pubkey()),
        writer_balance_before + buffer_rent,
    );
}

#[test]
fn flash_fulfill_intent_hash_consumes_append_built_buffer() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;

    let mut payload = Vec::new();
    payload.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    payload.extend_from_slice(&route.try_to_vec().unwrap());
    payload.extend_from_slice(&reward.try_to_vec().unwrap());

    let split = payload.len() / 2;
    let first_chunk = payload[..split].to_vec();
    let second_chunk = payload[split..].to_vec();

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, first_chunk)
        .unwrap();
    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, second_chunk)
        .unwrap();

    let writer_balance_before = ctx.balance(&writer.pubkey());
    let buffer_rent = ctx.balance(&buffer);

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        Some(writer.pubkey()),
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
    assert!(ctx.account::<FlashFulfillIntentAccount>(&buffer).is_none());
    assert_eq!(
        ctx.balance(&writer.pubkey()),
        writer_balance_before + buffer_rent,
    );
}

#[test]
fn flash_fulfill_intent_hash_missing_buffer_fail() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let writer = ctx.payer.pubkey();
    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        Some(writer),
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
        None,
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
        None,
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
        None,
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

#[test]
fn flash_fulfill_squatter_buffer_does_not_affect_writer() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

    let mallory = Keypair::new();
    ctx.airdrop(&mallory.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let mallory_buffer = FlashFulfillIntentAccount::pda(&mallory.pubkey(), &intent_hash_value).0;
    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&mallory, mallory_buffer, route.clone(), reward.clone())
        .unwrap();

    let writer = ctx.payer.insecure_clone();
    let writer_buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &intent_hash_value).0;
    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&writer, writer_buffer, route.clone(), reward.clone())
        .unwrap();

    assert_ne!(mallory_buffer, writer_buffer);

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        Some(writer.pubkey()),
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
    assert!(ctx
        .account::<FlashFulfillIntentAccount>(&writer_buffer)
        .is_none());
    assert!(ctx
        .account::<FlashFulfillIntentAccount>(&mallory_buffer)
        .is_some());
}

#[test]
fn flash_fulfill_intent_hash_rejects_wrong_writer() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

    let alice = ctx.payer.insecure_clone();
    let alice_buffer = FlashFulfillIntentAccount::pda(&alice.pubkey(), &intent_hash_value).0;
    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&alice, alice_buffer, route.clone(), reward.clone())
        .unwrap();

    let mallory = Keypair::new();
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill_explicit_buffer(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        Some(mallory.pubkey()),
        &route,
        &reward,
        claimant,
        Some(alice_buffer),
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::InvalidFlashFulfillIntentAccount
    )));
    assert!(ctx
        .account::<FlashFulfillIntentAccount>(&alice_buffer)
        .is_some());
}

#[test]
fn flash_fulfill_intent_hash_rejects_mismatched_intent_hash() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_a = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

    let alice = ctx.payer.insecure_clone();
    let alice_buffer = FlashFulfillIntentAccount::pda(&alice.pubkey(), &intent_hash_a).0;
    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&alice, alice_buffer, route.clone(), reward.clone())
        .unwrap();

    let mut intent_hash_b_bytes: [u8; 32] = intent_hash_a.into();
    intent_hash_b_bytes[0] ^= 0x01;
    let intent_hash_b = intent_hash_b_bytes.into();

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill_explicit_buffer(
        FlashFulfillIntent::IntentHash(intent_hash_b),
        Some(alice.pubkey()),
        &route,
        &reward,
        claimant,
        Some(alice_buffer),
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::InvalidFlashFulfillIntentAccount
    )));
    assert!(ctx
        .account::<FlashFulfillIntentAccount>(&alice_buffer)
        .is_some());
}

#[test]
fn flash_fulfill_intent_hash_rejects_buffer_content_hash_mismatch() {
    let (mut ctx, route, reward, _) = setup();
    let real_intent_hash = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

    let mut fake_intent_hash_bytes: [u8; 32] = real_intent_hash.into();
    fake_intent_hash_bytes[0] ^= 0x01;
    let fake_intent_hash = fake_intent_hash_bytes.into();

    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let fake_buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &fake_intent_hash).0;

    let mut payload = Vec::new();
    payload.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    payload.extend_from_slice(&route.try_to_vec().unwrap());
    payload.extend_from_slice(&reward.try_to_vec().unwrap());

    let split = payload.len() / 2;
    let first_chunk = payload[..split].to_vec();
    let second_chunk = payload[split..].to_vec();

    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, fake_intent_hash, first_chunk)
        .unwrap();
    ctx.flash_fulfiller()
        .append_flash_fulfill_intent_chunk(&writer, fake_intent_hash, second_chunk)
        .unwrap();

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill_explicit_buffer(
        FlashFulfillIntent::IntentHash(fake_intent_hash),
        Some(writer.pubkey()),
        &route,
        &reward,
        claimant,
        Some(fake_buffer),
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::InvalidFlashFulfillIntentAccount
    )));
    assert!(ctx
        .account::<FlashFulfillIntentAccount>(&fake_buffer)
        .is_some());
}

#[test]
fn flash_fulfill_inline_variant_works_without_buffer() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill_explicit_buffer(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        None,
        &route,
        &reward,
        claimant,
        None,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_ok_and(common::contains_cpi_event(FlashFulfilled {
        intent_hash: intent_hash_value,
        claimant,
        native_fee: reward.native_amount - route.native_amount,
    })));
    assert!(ctx
        .account::<WithdrawnMarker>(&WithdrawnMarker::pda(&intent_hash_value).0)
        .is_some());
    assert!(ctx
        .account::<FulfillMarker>(&FulfillMarker::pda(&intent_hash_value).0)
        .is_some());
}

#[test]
fn flash_fulfill_intent_hash_requires_buffer() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill_explicit_buffer(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        None,
        &route,
        &reward,
        claimant,
        None,
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::InvalidFlashFulfillIntentAccount
    )));
}

#[test]
fn flash_fulfill_inline_variant_does_not_close_passed_buffer() {
    let (mut ctx, route, reward, _) = setup();
    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());

    let alice = Keypair::new();
    ctx.airdrop(&alice.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let alice_buffer = FlashFulfillIntentAccount::pda(&alice.pubkey(), &intent_hash_value).0;
    ctx.flash_fulfiller()
        .set_flash_fulfill_intent(&alice, alice_buffer, route.clone(), reward.clone())
        .unwrap();

    let alice_buffer_lamports_before = ctx.balance(&alice_buffer);

    let mallory = Keypair::new();
    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let result = ctx.flash_fulfiller().flash_fulfill_explicit_buffer(
        FlashFulfillIntent::Intent {
            route: route.clone(),
            reward: reward.clone(),
        },
        Some(mallory.pubkey()),
        &route,
        &reward,
        claimant,
        Some(alice_buffer),
        claimant_ata_metas,
        vec![],
    );

    assert!(result.is_ok());
    assert!(ctx
        .account::<FlashFulfillIntentAccount>(&alice_buffer)
        .is_some());
    assert_eq!(ctx.balance(&alice_buffer), alice_buffer_lamports_before);
    assert_eq!(ctx.balance(&mallory.pubkey()), 0);
}

#[test]
fn flash_fulfill_large_route_consumes_without_oom() {
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

    let edge_account = Pubkey::new_unique();
    let unique_dummies: Vec<Pubkey> = (0..3).map(|_| Pubkey::new_unique()).collect();
    let middle_accounts: Vec<Pubkey> = (0..30)
        .map(|index| unique_dummies[index % unique_dummies.len()])
        .collect();

    let target = spl_noop::ID.to_bytes().into();
    let build_call = |data: Vec<u8>, accounts: Vec<Pubkey>| Call {
        target,
        data: CalldataWithAccounts {
            calldata: Calldata {
                data,
                account_count: accounts.len() as u8,
            },
            accounts: accounts
                .into_iter()
                .map(|pubkey| SerializableAccountMeta {
                    pubkey,
                    is_signer: false,
                    is_writable: false,
                })
                .collect(),
        }
        .try_to_vec()
        .unwrap(),
    };

    route.calls = vec![
        build_call(vec![0u8], vec![edge_account]),
        build_call(vec![0u8; 8], middle_accounts.clone()),
        build_call(vec![0u8], vec![edge_account]),
    ];

    let encoded_route_size = route.try_to_vec().unwrap().len();
    assert!(
        encoded_route_size >= 1300,
        "expected jupiter-shape route to encode to >=1300 bytes, got {encoded_route_size}",
    );

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

    let writer = Keypair::new();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(2.0))
        .unwrap();

    let mut payload = Vec::new();
    payload.extend_from_slice(FlashFulfillIntentAccount::DISCRIMINATOR);
    payload.extend_from_slice(&route.try_to_vec().unwrap());
    payload.extend_from_slice(&reward.try_to_vec().unwrap());

    payload.chunks(900).for_each(|chunk| {
        ctx.flash_fulfiller()
            .append_flash_fulfill_intent_chunk(&writer, intent_hash_value, chunk.to_vec())
            .unwrap();
    });

    let claimant = Pubkey::new_unique();
    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
    });

    let claimant_ata_metas = claimant_atas(&ctx, &reward, claimant);
    let call_accounts: Vec<AccountMeta> = std::iter::once(edge_account)
        .chain(middle_accounts.iter().copied())
        .chain(std::iter::once(edge_account))
        .chain(std::iter::once(spl_noop::ID))
        .map(|pubkey| AccountMeta::new_readonly(pubkey, false))
        .collect();

    let result = ctx.flash_fulfiller().flash_fulfill(
        FlashFulfillIntent::IntentHash(intent_hash_value),
        Some(writer.pubkey()),
        &route,
        &reward,
        claimant,
        claimant_ata_metas,
        call_accounts,
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
}
