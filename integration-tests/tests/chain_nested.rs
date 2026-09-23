//! Nested recipients are route data; all token movement remains local.
mod common;
#[path = "../../programs/intent-chainer/testdata/fixture_parser.rs"]
mod fixtures;

use anchor_lang::prelude::AccountMeta;
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id as ata;
use common::intent_chainer_context::{keccak, ChainedIntent};
use common::{
    contains_cpi_event, contains_event, is_error, order_buffer_context as staging, Context,
};
use eco_svm_std::prover::Proof;
use intent_chainer::events::{IntentChained, OrderAnnounced};
use intent_chainer::instructions::ChainerError;
use intent_chainer::state::{escrow_authority_pda, vault_pda, OrderBuffer};
use intent_chainer::types::*;
use portal::events::IntentPublished;
use portal::state::WithdrawnMarker;
use portal::types::intent_hash;
use serde_json::Value;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

fn setup(case: &Value) -> (Context, Order, u64, Vec<u8>) {
    let mut ctx = Context::default();
    ctx.compute_limit = 1_400_000;
    let mint = Pubkey::new_unique();
    ctx.set_mint_account(&mint);
    let mut order = ctx.intent_chainer().svm_order(
        mint,
        Pubkey::new_unique(),
        local_prover::ID,
        case["scale"].as_str().unwrap().parse().unwrap(),
        1,
    );
    order.template = fixtures::program(&case["program"]);
    order.destination = 8453;
    order.require_publish = true;
    (
        ctx,
        order,
        case["input"].as_str().unwrap().parse().unwrap(),
        fixtures::hex(case["expected_route"].as_str().unwrap()),
    )
}

/// Accounts derived from INDEPENDENT fixture bytes, not the new renderer.
fn resolve(ctx: &mut Context, order: &Order, input: u64, route_bytes: &[u8]) -> ChainedIntent {
    let order_commitment = order.hash();
    let escrow_authority = escrow_authority_pda(&order_commitment).0;
    let mut reward = order.reward.clone();
    reward.tokens[0].amount = input;
    let intent_hash = intent_hash(order.destination, &keccak(route_bytes), &reward.hash());
    let vault = vault_pda(&order.portal, &intent_hash).0;
    ChainedIntent {
        order: order.clone(),
        order_commitment,
        escrow_authority,
        escrow_ata: ata(&escrow_authority, &order.base_mint, &ctx.token_program),
        // This test never fulfills the root on SVM; it executes on a remote VM.
        route: ctx.rand_intent().1,
        reward,
        intent_hash,
        vault,
        vault_ata: ata(&vault, &order.base_mint, &ctx.token_program),
        amount_out: scale_amount(input as u128, order.scale).unwrap(),
    }
}

fn chain_instruction(ctx: &Context, c: &ChainedIntent) -> Instruction {
    Instruction {
        program_id: intent_chainer::ID,
        accounts: intent_chainer::accounts::Chain {
            payer: ctx.payer.pubkey(),
            escrow_authority: c.escrow_authority,
            escrow_ata: c.escrow_ata,
            base_mint: c.order.base_mint,
            vault: c.vault,
            vault_ata: c.vault_ata,
            portal_program: c.order.portal,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: intent_chainer::instruction::Chain {
            args: intent_chainer::instructions::ChainArgs {
                order: c.order.clone(),
                publish: false,
            },
        }
        .data(),
    }
}

fn published(c: &ChainedIntent, route: Vec<u8>) -> IntentPublished {
    IntentPublished::new(c.intent_hash, c.order.destination, route, c.reward.clone())
}

#[test]
fn prefunded_vaults_accept_the_full_transfer_through_both_transports() {
    for staged in [false, true] {
        for prefund in [0, 1, 1_234_566, 1_234_567, 2_469_134] {
            let mut ctx = Context::default();
            let input = 1_234_567;
            let mint = Pubkey::new_unique();
            ctx.set_mint_account(&mint);
            let recipient = Pubkey::new_unique();
            let mut order =
                ctx.intent_chainer()
                    .svm_order(mint, recipient, local_prover::ID, WAD, 1);
            order.require_publish = true;
            let c = ctx.intent_chainer().resolve(&order, input, recipient);
            ctx.intent_chainer().seed_escrow(&c, input);
            ctx.airdrop_token_ata(&mint, &c.vault, prefund);
            let meta = if staged {
                let payer = ctx.payer.insecure_clone();
                let buffer = staging::upload(
                    &mut ctx,
                    &payer,
                    [0x51; 32],
                    order.hash(),
                    &borsh::to_vec(&order).unwrap(),
                );
                staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
                    .0
                    .unwrap();
                let ix = staging::execute(chain_instruction(&ctx, &c), buffer, false);
                staging::send(&mut ctx, &payer, &[ix]).0.unwrap()
            } else {
                ctx.intent_chainer().chain(&c, false).unwrap()
            };
            assert_eq!(ctx.token_balance(&c.escrow_ata), 0);
            assert_eq!(ctx.token_balance(&c.vault_ata), prefund + input);
            assert_eq!(
                c.reward.tokens[0].amount, input,
                "prefunding does not enlarge the reward"
            );
            assert!(contains_event(published(
                &c,
                borsh::to_vec(&c.route).unwrap()
            ))(meta.clone()));
            assert!(contains_event(IntentChained::new(
                c.intent_hash,
                c.order_commitment,
                c.vault,
                mint,
                input,
                input as u128,
                order.destination,
                c.route.hash(),
                true,
            ))(meta));
        }
    }
}

#[test]
fn transfer_fee_push_shortfall_rolls_back_even_with_prefunding_and_staging() {
    use anchor_spl::token_2022::spl_token_2022::extension::transfer_fee::TransferFeeAmount;
    use anchor_spl::token_2022::spl_token_2022::extension::{
        BaseStateWithExtensions, StateWithExtensions,
    };
    use anchor_spl::token_2022::spl_token_2022::instruction::transfer_checked;
    use anchor_spl::token_2022::spl_token_2022::state::Account;

    let input = 1_234_567u64;
    let fee = input.div_ceil(100); // 100 basis points, uncapped.
    for staged in [false, true] {
        // The fee-sized prefund used to mask the short delivery in a total-balance
        // check. Also cover ATA creation, an empty ATA, dust, equal and larger balances.
        for prefund in [
            None,
            Some(0),
            Some(1),
            Some(fee),
            Some(input),
            Some(2 * input),
        ] {
            let mut ctx = Context::new_with_token_2022();
            let mint = ctx.create_transfer_fee_mint(100, u64::MAX);
            let payer = ctx.payer.insecure_clone();
            let recipient = Pubkey::new_unique();
            ctx.airdrop_token_ata(&mint, &payer.pubkey(), input);
            ctx.airdrop_token_ata(&mint, &recipient, 0);
            let recipient_ata = ata(&recipient, &mint, &ctx.token_program);

            // Control: this exact mint's real Token-2022 processor withholds the
            // expected fee. A plain mint or broken setup must not satisfy the test.
            let transfer = transfer_checked(
                &ctx.token_program,
                &ata(&payer.pubkey(), &mint, &ctx.token_program),
                &mint,
                &recipient_ata,
                &payer.pubkey(),
                &[],
                input,
                6,
            )
            .unwrap();
            staging::send(&mut ctx, &payer, &[transfer]).0.unwrap();
            let recipient_account = ctx.get_account(&recipient_ata).unwrap();
            let recipient_state =
                StateWithExtensions::<Account>::unpack(&recipient_account.data).unwrap();
            assert_eq!(recipient_state.base.amount, input - fee);
            assert_eq!(
                u64::from(
                    recipient_state
                        .get_extension::<TransferFeeAmount>()
                        .unwrap()
                        .withheld_amount
                ),
                fee
            );

            let mut order =
                ctx.intent_chainer()
                    .svm_order(mint, recipient_ata, local_prover::ID, WAD, 1);
            order.require_publish = true;
            let c = ctx.intent_chainer().resolve(&order, input, recipient_ata);
            ctx.intent_chainer().seed_escrow(&c, input);
            if let Some(amount) = prefund {
                ctx.airdrop_token_ata(&mint, &c.vault, amount);
            }
            let escrow_before = ctx.get_account(&c.escrow_ata);
            let vault_before = ctx.get_account(&c.vault_ata);
            let mint_before = ctx.get_account(&mint);
            let err = if staged {
                let buffer = staging::upload(
                    &mut ctx,
                    &payer,
                    [0x52; 32],
                    order.hash(),
                    &borsh::to_vec(&order).unwrap(),
                );
                staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
                    .0
                    .unwrap();
                let buffer_before = ctx.get_account(&buffer);
                let ix = staging::execute(chain_instruction(&ctx, &c), buffer, false);
                let err = staging::send(&mut ctx, &payer, &[ix]).0.unwrap_err();
                assert_eq!(ctx.get_account(&buffer), buffer_before);
                err
            } else {
                ctx.intent_chainer().chain(&c, false).unwrap_err()
            };
            assert_eq!(ctx.token_balance(&c.escrow_ata), input);
            assert_eq!(ctx.get_account(&c.escrow_ata), escrow_before);
            assert_eq!(
                ctx.get_account(&c.vault_ata),
                vault_before,
                "including withheld fees and ATA creation"
            );
            assert_eq!(ctx.get_account(&mint), mint_before);
            assert!(!contains_event(published(
                &c,
                borsh::to_vec(&c.route).unwrap()
            ))(err.meta.clone()));
            assert!(!contains_event(IntentChained::new(
                c.intent_hash,
                c.order_commitment,
                c.vault,
                mint,
                input,
                input as u128,
                order.destination,
                c.route.hash(),
                true,
            ))(err.meta.clone()));
            assert!(is_error(ChainerError::PushShortfall)(err));
        }
    }
}

#[test]
fn wide_initial_output_is_scaled_without_narrowing_on_sbf() {
    let (mut ctx, mut order, _, _) = setup(&fixtures::nested()["cases"][0]);
    let input = 1_000_000_001u64;
    let output = 1_000_000_001_000_000_000_000u128;
    assert!(output > u64::MAX as u128);
    order.scale = 10u128.pow(30);
    let downscale = Amount {
        scale: 1_000_000,
        ..Amount::output(8, true)
    };
    let single = |amount| Template {
        segments: vec![vec![], vec![]],
        items: vec![Item::Amount(amount)],
    };
    order.template.vaults[0].route = single(Amount::output(32, false));
    order.template.vaults[0].reward = single(downscale.clone());
    order.template.route = Template {
        segments: vec![vec![]; 5],
        items: vec![
            Item::Amount(downscale),
            Item::Amount(Amount::output(32, false)),
            Item::Amount(Amount {
                source: AmountSource::Input,
                scale: WAD / 2,
                width: 8,
                little_endian: true,
            }),
            Item::Vault(0),
        ],
    };
    // Arithmetic-only node bodies, not executable remote Route/Reward encodings.
    // Independent ethers packed Keccak + getCreate2Address with fixture case 0:
    // route = uint256_be(output), reward = uint64_le(input), destination = 480.
    // Node hash = ece60b92c20e701ca75d3f7cc2af021e0fb12c25130388abfd5c60cb272f8b6e.
    let mut expected = input.to_le_bytes().to_vec();
    expected.extend_from_slice(&[0; 16]);
    expected.extend_from_slice(&output.to_be_bytes());
    expected.extend_from_slice(&500_000_001u64.to_le_bytes());
    expected.extend_from_slice(&fixtures::hex(
        "000000000000000000000000ef22ee696a6c8b84fd12981e31b452d54324dc12",
    ));
    let c = resolve(&mut ctx, &order, input, &expected);
    ctx.intent_chainer().seed_escrow(&c, input);
    let meta = ctx.intent_chainer().chain(&c, false).unwrap();
    assert!(contains_event(published(&c, expected.clone()))(
        meta.clone()
    ));
    assert!(contains_event(IntentChained::new(
        c.intent_hash,
        c.order_commitment,
        c.vault,
        order.base_mint,
        input,
        output,
        order.destination,
        keccak(&expected),
        true,
    ))(meta));
    assert_eq!(ctx.token_balance(&c.escrow_ata), 0);
    assert_eq!(ctx.token_balance(&c.vault_ata), input);
}

#[test]
fn all_cross_vm_fixtures_fund_only_the_local_vault_and_publish_exact_bytes() {
    for case in fixtures::nested()["cases"].as_array().unwrap() {
        let (mut ctx, order, input, route) = setup(case);
        let c = resolve(&mut ctx, &order, input, &route);
        ctx.intent_chainer().seed_escrow(&c, input);
        let mut remote_accounts = vec![];
        for node in case["nodes"].as_array().unwrap() {
            let remote = Pubkey::new_from_array(fixtures::bytes(&node["recipient"]));
            assert_ne!(c.vault_ata, remote);
            remote_accounts.push((remote, ctx.get_account(&remote)));
        }
        let announcement = ctx
            .intent_chainer()
            .announce_order(&order)
            .unwrap_or_else(|e| panic!("{}: {}", case["name"], e.err));
        assert!(contains_cpi_event(OrderAnnounced::new(
            c.order_commitment,
            c.escrow_authority,
            order.clone()
        ))(announcement));
        let meta = ctx
            .intent_chainer()
            .chain(&c, false)
            .unwrap_or_else(|e| panic!("{}: {} {:?}", case["name"], e.err, e.meta.logs));
        assert!(contains_event(published(&c, route.clone()))(meta.clone()));
        assert!(contains_event(IntentChained::new(
            c.intent_hash,
            c.order_commitment,
            c.vault,
            order.base_mint,
            input,
            c.amount_out,
            order.destination,
            keccak(&route),
            true
        ))(meta.clone()));
        assert_eq!(ctx.token_balance(&c.escrow_ata), 0);
        assert_eq!(ctx.token_balance(&c.vault_ata), input);
        for (remote, before) in remote_accounts {
            assert_eq!(ctx.get_account(&remote), before);
        }
        println!(
            "{}: order={} root={} CU={} logs={}",
            case["name"],
            borsh::to_vec(&order).unwrap().len(),
            route.len(),
            meta.compute_units_consumed,
            meta.logs.iter().map(String::len).sum::<usize>()
        );
    }
}

#[test]
fn nested_stale_accounts_fail_atomically_then_the_same_order_retries_after_expiry() {
    let data = fixtures::nested();
    let (mut ctx, order, input, route) = setup(&data["cases"][6]); // Solana remote recipient
    let stale = resolve(&mut ctx, &order, input, &route);
    let new_route = fixtures::hex(data["cases"][7]["expected_route"].as_str().unwrap());
    let current = resolve(&mut ctx, &order, input + 1, &new_route);
    ctx.intent_chainer().seed_escrow(&stale, input + 1);
    assert_ne!(stale.vault, current.vault);
    assert_eq!(stale.escrow_ata, current.escrow_ata);
    let failed = ctx.intent_chainer().chain(&stale, false).unwrap_err();
    assert!(is_error(ChainerError::InvalidVault)(failed));
    assert_eq!(ctx.token_balance(&stale.escrow_ata), input + 1);
    assert!(ctx.get_account(&stale.vault_ata).is_none());
    assert!(ctx.get_account(&current.vault_ata).is_none());
    ctx.warp_to_timestamp(order.reward.deadline as i64 + 1);
    let meta = ctx.intent_chainer().chain(&current, false).unwrap();
    assert!(contains_event(published(&current, new_route.clone()))(meta));
    let creator = order.reward.creator;
    ctx.airdrop_token_ata(&order.base_mint, &creator, 0);
    let creator_ata = ata(&creator, &order.base_mint, &ctx.token_program);
    ctx.portal()
        .refund_intent(
            order.destination,
            current.reward.clone(),
            current.vault,
            keccak(&new_route),
            Proof::pda(&current.intent_hash, &order.reward.prover).0,
            WithdrawnMarker::pda(&current.intent_hash).0,
            creator,
            [
                AccountMeta::new(current.vault_ata, false),
                AccountMeta::new(creator_ata, false),
                AccountMeta::new_readonly(order.base_mint, false),
            ],
        )
        .unwrap();
    assert_eq!(ctx.token_balance(&creator_ata), input + 1);
    assert!(ctx.get_account(&current.vault_ata).is_none());
}

#[test]
fn nested_order_mutation_cannot_use_foreign_escrow() {
    let data = fixtures::nested();
    let (mut ctx, order, input, route) = setup(&data["cases"][8]);
    let c = resolve(&mut ctx, &order, input, &route);
    ctx.intent_chainer().seed_escrow(&c, input);
    for field in 0..7 {
        let mut changed = order.clone();
        match field {
            0 => changed.template.vaults[0].reward.segments[0][0] ^= 1,
            1 => changed.template.vaults[0].route.segments[0][0] ^= 1,
            2 => changed.template.vaults[1].destination += 1,
            3 => changed.template.route.items[0] = Item::Vault(0),
            4 => {
                if let Derivation::Solana(config) = &mut changed.template.vaults[1].derivation {
                    config.portal = Pubkey::new_unique();
                }
            }
            5 => {
                if let Item::Amount(amount) = &mut changed.template.vaults[0].reward.items[0] {
                    amount.scale += 1;
                }
            }
            _ => changed.portal = Pubkey::new_unique(),
        }
        let mut wrong = resolve(&mut ctx, &changed, input, &route);
        wrong.escrow_authority = c.escrow_authority;
        wrong.escrow_ata = c.escrow_ata;
        // Portal substitution needs an executable account to reach the commitment
        // check; this field is separately covered by the existing multi-Portal test.
        if field == 6 {
            ctx.add_program(
                changed.portal,
                include_bytes!("../../target/deploy/portal.so"),
            )
            .unwrap();
        }
        assert!(ctx
            .intent_chainer()
            .chain(&wrong, false)
            .is_err_and(is_error(ChainerError::InvalidEscrowAuthority)));
        assert_eq!(ctx.token_balance(&c.escrow_ata), input);
    }
}

#[test]
fn nested_transfer_and_publish_failures_preserve_custody() {
    let data = fixtures::nested();
    for failure in [2, 3] {
        let (mut ctx, mut order, input, route) = setup(&data["cases"][0]);
        if failure == 3 {
            // An executable, committed target that has no Portal publish handler.
            // Its CPI fails AFTER the token transfer, which must roll back too.
            order.portal = local_prover::ID;
        }
        let c = resolve(&mut ctx, &order, input, &route);
        ctx.intent_chainer().seed_escrow(&c, input);
        if failure == 2 {
            // A frozen source is a real token-program rejection after local
            // account validation. ATA creation must roll back with transfer.
            use solana_sdk::program_pack::Pack;
            let mut account = ctx.get_account(&c.escrow_ata).unwrap();
            let mut token =
                anchor_spl::token::spl_token::state::Account::unpack(&account.data).unwrap();
            token.state = anchor_spl::token::spl_token::state::AccountState::Frozen;
            anchor_spl::token::spl_token::state::Account::pack(token, &mut account.data).unwrap();
            ctx.set_account(c.escrow_ata, account).unwrap();
        }
        let vault_before = ctx.get_account(&c.vault_ata);
        let err = ctx.intent_chainer().chain(&c, false).unwrap_err();
        // Native staging must reach the identical settlement/transfer/publication
        // failure, with no consumed buffer or custody changes on rollback.
        let payer = ctx.payer.insecure_clone();
        let buffer = staging::upload(
            &mut ctx,
            &payer,
            [8; 32],
            order.hash(),
            &borsh::to_vec(&order).unwrap(),
        );
        staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
            .0
            .unwrap();
        let buffer_before = ctx.get_account(&buffer).unwrap();
        let ix = staging::execute(chain_instruction(&ctx, &c), buffer, false);
        let native_error = staging::send(&mut ctx, &payer, &[ix]).0.unwrap_err();
        match (&err.err, &native_error.err) {
            (
                solana_sdk::transaction::TransactionError::InstructionError(_, direct),
                solana_sdk::transaction::TransactionError::InstructionError(_, staged),
            ) => assert_eq!(direct, staged),
            errors => panic!("unexpected failures {errors:?}"),
        }
        assert_eq!(ctx.get_account(&buffer).unwrap(), buffer_before);
        if failure == 3 {
            let mut transfer_checked = vec![12];
            transfer_checked.extend_from_slice(&input.to_le_bytes());
            transfer_checked.push(6); // fixture mint decimals
            assert!(err.meta.inner_instructions.iter().flatten().any(|ix| {
                ix.instruction.data == transfer_checked && ix.instruction.accounts.len() == 4
            }));
            assert!(is_error(
                anchor_lang::error::ErrorCode::InstructionFallbackNotFound
            )(err));
        }
        assert_eq!(ctx.token_balance(&c.escrow_ata), input);
        assert_eq!(ctx.get_account(&c.vault_ata), vault_before);
    }
}

#[test]
fn malformed_nested_wire_data_cannot_move_funded_escrow() {
    let (mut ctx, order, input, route) = setup(&fixtures::nested()["cases"][6]);
    let c = resolve(&mut ctx, &order, input, &route);
    ctx.intent_chainer().seed_escrow(&c, input);
    let original = chain_instruction(&ctx, &c);
    let node = &order.template.vaults[0];
    // Exact offsets from the documented Borsh schema, not magic searched bytes.
    let node_route = 8 + 32 + 32 + 8 + 4 + 8;
    let item = node_route + borsh::to_vec(&node.route.segments).unwrap().len() + 4;
    let derivation = node_route
        + borsh::to_vec(&node.route).unwrap().len()
        + borsh::to_vec(&node.reward).unwrap().len();
    for corruption in 0..6 {
        let mut ix = original.clone();
        match corruption {
            0 => ix.data[derivation] = 2, // unknown derivation tag
            1 => ix.data[item] = 2,       // unknown item tag
            2 => ix.data[item + 1] = 2,   // unknown source tag
            3 => {
                ix.data.splice(derivation + 97..derivation + 97, [254, 255]);
            } // no bump fields
            4 => {
                ix.data.pop();
            } // missing call.publish
            _ => ix.data[8 + 72..8 + 76].copy_from_slice(&u32::MAX.to_le_bytes()),
        }
        let result = ctx.send_template_instruction(ix);
        assert!(
            result.is_err_and(is_error(
                anchor_lang::error::ErrorCode::InstructionDidNotDeserialize
            )),
            "corruption {corruption}"
        );
        assert_eq!(ctx.token_balance(&c.escrow_ata), input);
        assert!(ctx.get_account(&c.vault_ata).is_none());
    }
}

#[test]
fn invalid_nested_programs_are_rejected_by_announce_and_chain_before_transfer() {
    let cases = fixtures::nested();
    for invalid in 0..13 {
        let (mut ctx, mut order, input, route) = setup(&cases["cases"][0]);
        match invalid {
            0 => {
                if let Derivation::Evm(c) = &mut order.template.vaults[0].derivation {
                    c.portal = [0; 20];
                }
            }
            1 => {
                if let Derivation::Evm(c) = &mut order.template.vaults[0].derivation {
                    c.prefix = 0;
                }
            }
            2 => {
                if let Derivation::Evm(c) = &mut order.template.vaults[0].derivation {
                    c.implementation = [0; 20];
                }
            }
            3 => {
                if let Derivation::Evm(c) = &mut order.template.vaults[0].derivation {
                    c.init_code_hash = [0; 32];
                }
            }
            4..=6 => {
                let mut config = SolanaDerivation {
                    portal: portal::ID,
                    token_program: ctx.token_program,
                    mint: order.base_mint,
                };
                if invalid == 4 {
                    config.portal = Pubkey::default();
                } else if invalid == 5 {
                    config.token_program = Pubkey::default();
                } else {
                    config.mint = Pubkey::default();
                }
                order.template.vaults[0].derivation = Derivation::Solana(config);
            }
            7 => order.template.vaults[0].route.items[0] = Item::Vault(0),
            8 => order.template.vaults[0].reward.items[0] = Item::Vault(1),
            9 => order.template.route.items[3] = Item::Vault(255),
            10 => {
                if let Item::Amount(a) = &mut order.template.vaults[0].reward.items[0] {
                    a.scale = 0;
                }
            }
            11 => order.template.vaults[0].reward.segments.push(vec![]),
            _ => order.template.vaults[0].reward.segments[0].extend_from_slice(&[0; 1024]),
        }
        let c = resolve(&mut ctx, &order, input, &route);
        ctx.intent_chainer().seed_escrow(&c, input);
        let chain_error = ctx.intent_chainer().chain(&c, false).unwrap_err();
        let announce_error = ctx.intent_chainer().announce_order(&order).unwrap_err();
        assert_eq!(
            chain_error.err, announce_error.err,
            "same validation gate, case {invalid}"
        );
        assert_eq!(ctx.token_balance(&c.escrow_ata), input);
        assert!(ctx.get_account(&c.vault_ata).is_none());
    }
}

#[test]
fn near_maximum_root_with_eight_solana_nodes_publishes_completely() {
    let (mut ctx, base, _, _) = setup(&fixtures::nested()["cases"][6]);
    let mut order = base;
    let mut node = order.template.vaults[0].clone();
    node.route = Template::literal(vec![1]);
    node.reward = Template::literal(vec![2]);
    order.template.vaults = vec![node; MAX_VAULTS];
    let root = vec![0xa5; MAX_RENDERED_BYTES - 2 * MAX_VAULTS];
    order.template.route = Template::literal(root.clone());
    let c = resolve(&mut ctx, &order, 1000, &root);
    ctx.intent_chainer().seed_escrow(&c, 1000);
    let meta = ctx
        .intent_chainer()
        .chain(&c, false)
        .unwrap_or_else(|e| panic!("{} {:?}", e.err, e.meta.logs));
    assert!(contains_event(published(&c, root))(meta));
    assert_eq!(ctx.token_balance(&c.vault_ata), 1000);
}

/// Maximum nodes AND items AND encoded bytes AND aggregate rendered bytes.
/// Non-root templates exercise allocation/hashing even when the root ignores them.
fn maximum_order(base: &Order) -> (Order, Vec<u8>) {
    let sdk = fixtures::solana();
    let tiny = Template {
        segments: vec![vec![]; MAX_ITEMS + 1],
        items: vec![Item::Amount(Amount::output(1, true)); MAX_ITEMS],
    };
    let node = Vault {
        destination: 480,
        route: tiny.clone(),
        reward: tiny.clone(),
        derivation: Derivation::Solana(SolanaDerivation {
            portal: Pubkey::new_from_array(fixtures::bytes(&sdk["portal"])),
            token_program: Pubkey::new_from_array(fixtures::bytes(&sdk["tokenProgram"])),
            mint: Pubkey::new_from_array(fixtures::bytes(&sdk["mint"])),
        }),
    };
    let mut order = base.clone();
    order.scale = WAD;
    order.template = TemplateProgram {
        vaults: vec![node; MAX_VAULTS],
        route: tiny,
    };
    // References are smaller on the wire but each renders to 32 bytes. Replace
    // just enough amount items to fit, preserving maximal allocation cardinality.
    'fit: for n in 1..MAX_VAULTS {
        for reward in [false, true] {
            for i in 0..MAX_ITEMS {
                if borsh::to_vec(&order).unwrap().len() <= MAX_ORDER_BYTES {
                    break 'fit;
                }
                let template = if reward {
                    &mut order.template.vaults[n].reward
                } else {
                    &mut order.template.vaults[n].route
                };
                template.items[i] = Item::Vault((n - 1) as u8);
            }
        }
    }
    let pad = MAX_ORDER_BYTES - borsh::to_vec(&order).unwrap().len();
    order.template.route.segments[0] = vec![0x99; pad];
    let length = |t: &Template| {
        t.segments.iter().map(Vec::len).sum::<usize>()
            + t.items
                .iter()
                .map(|i| match i {
                    Item::Amount(a) => a.width as usize,
                    Item::Vault(_) => 32,
                })
                .sum::<usize>()
    };
    let used = length(&order.template.route)
        + order
            .template
            .vaults
            .iter()
            .map(|n| length(&n.route) + length(&n.reward))
            .sum::<usize>();
    assert!(used <= MAX_RENDERED_BYTES);
    let mut remaining = MAX_RENDERED_BYTES - used;
    for node in &mut order.template.vaults {
        for template in [&mut node.route, &mut node.reward] {
            for item in &mut template.items {
                if let Item::Amount(amount) = item {
                    let extra = remaining.min(31);
                    amount.width += extra as u8;
                    remaining -= extra;
                }
            }
        }
    }
    assert_eq!(remaining, 0);
    assert_eq!(borsh::to_vec(&order).unwrap().len(), MAX_ORDER_BYTES);
    order.validate_template().unwrap();
    let mut root = vec![0x99; pad];
    root.extend_from_slice(&[1; MAX_ITEMS]);
    (order, root)
}

#[test]
fn maximum_nested_shape_executes_and_its_full_announcement_survives_exhausted_logs() {
    let (mut ctx, base, _, _) = setup(&fixtures::nested()["cases"][0]);
    let (order, route) = maximum_order(&base);
    let c = resolve(&mut ctx, &order, 1, &route);
    let standalone = ctx
        .intent_chainer()
        .announce_order(&order)
        .unwrap_or_else(|e| panic!("{} {:?}", e.err, e.meta.logs));
    assert!(contains_cpi_event(OrderAnnounced::new(
        c.order_commitment,
        c.escrow_authority,
        order.clone()
    ))(standalone));
    // Intentionally exhaust normal logs BEFORE announce. The CPI event must
    // remain byte-exact in recorded inner instruction data, not a truncated log.
    let announce = Instruction {
        program_id: intent_chainer::ID,
        accounts: intent_chainer::accounts::AnnounceOrder {
            event_authority: Pubkey::find_program_address(
                &[b"__event_authority"],
                &intent_chainer::ID,
            )
            .0,
            program: intent_chainer::ID,
        }
        .to_account_metas(None),
        data: intent_chainer::instruction::AnnounceOrder {
            args: intent_chainer::instructions::AnnounceOrderArgs {
                order: order.clone(),
            },
        }
        .data(),
    };
    let staged_announce = ctx.stage_template_instruction(announce).0;
    let mut extra = vec![
        Instruction {
            program_id: staged_announce.program_id,
            accounts: vec![staged_announce.accounts[0].clone()],
            data: vec![255],
        },
        staged_announce,
    ];
    let payer = ctx.payer.insecure_clone();
    extra.insert(
        0,
        solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_limit(
            1_400_000,
        ),
    );
    let tx = solana_sdk::transaction::Transaction::new(
        &[&payer],
        solana_sdk::message::Message::new(&extra, Some(&payer.pubkey())),
        ctx.latest_blockhash(),
    );
    assert!(common::template_transport::transaction_size(&tx) <= 1232);
    let meta = ctx
        .send_transaction(tx)
        .unwrap_or_else(|e| panic!("{} {:?}", e.err, e.meta.logs));
    assert!(
        meta.logs.iter().any(|l| l.contains("Log truncated")),
        "must actually exhaust logs"
    );
    assert!(contains_cpi_event(OrderAnnounced::new(
        c.order_commitment,
        c.escrow_authority,
        order.clone()
    ))(meta.clone()));
    println!(
        "max announcement: order={} CU={} logs={}",
        MAX_ORDER_BYTES,
        meta.compute_units_consumed,
        meta.logs.iter().map(String::len).sum::<usize>()
    );
    ctx.intent_chainer().seed_escrow(&c, 1);
    let meta = ctx
        .send_template_instruction(chain_instruction(&ctx, &c))
        .unwrap_or_else(|e| panic!("{} {:?}", e.err, e.meta.logs));
    assert!(contains_event(published(&c, route))(meta.clone()));
    assert_eq!(ctx.token_balance(&c.vault_ata), 1);
    assert_eq!(ctx.token_balance(&c.escrow_ata), 0);
    println!(
        "max nested: nodes={} items/template={} aggregate={} order={} CU={} logs={}",
        MAX_VAULTS,
        MAX_ITEMS,
        MAX_RENDERED_BYTES,
        MAX_ORDER_BYTES,
        meta.compute_units_consumed,
        meta.logs.iter().map(String::len).sum::<usize>()
    );
}

/// A transport fixture, not a captured solver quote: complete downstream Portal
/// Borsh Route/Reward, a canonical Solana vault, and a root route-call envelope
/// containing both a runtime amount and recipient. Opaque call execution is not
/// part of this test (the cross-VM fixture tests cover independent rendering).
fn order_781(ctx: &mut Context) -> Order {
    use portal::types::{Call, Route};
    let mint = Pubkey::new_unique();
    ctx.set_mint_account(&mint);
    let mut order =
        ctx.intent_chainer()
            .svm_order(mint, Pubkey::new_unique(), local_prover::ID, WAD, 1);
    let downstream = Route {
        deadline: ctx.now() + 1800,
        salt: [0x72; 32].into(),
        portal: portal::ID.to_bytes().into(),
        native_amount: 0,
        tokens: vec![],
        calls: vec![],
    };
    let reward = borsh::to_vec(&order.reward).unwrap();
    order.template.vaults = vec![Vault {
        destination: eco_svm_std::CHAIN_ID,
        route: Template::literal(borsh::to_vec(&downstream).unwrap()),
        reward: Template {
            segments: vec![reward[..reward.len() - 8].to_vec(), vec![]],
            items: vec![Item::Amount(Amount {
                source: AmountSource::Input,
                ..Amount::output(8, true)
            })],
        },
        derivation: Derivation::Solana(SolanaDerivation {
            portal: portal::ID,
            token_program: ctx.token_program,
            mint,
        }),
    }];
    // Application call header: discriminator, version, chain, deadline, domain, finality.
    let mut payload = vec![0x35; 8];
    payload.push(1);
    payload.extend_from_slice(&480u64.to_le_bytes());
    payload.extend_from_slice(&downstream.deadline.to_le_bytes());
    payload.extend_from_slice(&6u32.to_le_bytes());
    payload.extend_from_slice(&1000u32.to_le_bytes());
    let prefix_len = payload.len();
    payload.extend_from_slice(&[0; 40]); // amount:u64 + mintRecipient:bytes32
    let root = Route {
        calls: vec![Call {
            target: [0x51; 32].into(),
            data: payload,
        }],
        ..downstream
    };
    let bytes = borsh::to_vec(&root).unwrap();
    let cut = bytes.len() - 40;
    assert_eq!(prefix_len, 33);
    order.template.route = Template {
        segments: vec![bytes[..cut].to_vec(), vec![], vec![]],
        items: vec![Item::Amount(Amount::output(8, true)), Item::Vault(0)],
    };
    order.require_publish = true;
    assert_eq!(borsh::to_vec(&order).unwrap().len(), 781);
    order
}

#[test]
fn native_staging_781_byte_order_fits_both_prepare_phases_and_chain() {
    let mut ctx = Context::default();
    ctx.compute_limit = 1_400_000;
    let order = order_781(&mut ctx);
    let input = 1000;
    let route = order.build_route(input).unwrap();
    let c = resolve(&mut ctx, &order, input, &route);
    let payer = ctx.payer.insecure_clone();
    let seed = Pubkey::new_unique().to_bytes();
    let buffer = OrderBuffer::pda(&payer.pubkey(), &seed).0;
    let bytes = borsh::to_vec(&order).unwrap();
    let create_ata = anchor_spl::associated_token::spl_associated_token_account::instruction::create_associated_token_account_idempotent(
        &payer.pubkey(), &c.escrow_authority, &order.base_mint, &ctx.token_program,
    );
    let direct_chain = chain_instruction(&ctx, &c);
    let direct_announce = Instruction {
        program_id: intent_chainer::ID,
        accounts: intent_chainer::accounts::AnnounceOrder {
            event_authority: intent_chainer::EVENT_AUTHORITY_AND_BUMP.0,
            program: intent_chainer::ID,
        }
        .to_account_metas(None),
        data: intent_chainer::instruction::AnnounceOrder {
            args: intent_chainer::instructions::AnnounceOrderArgs {
                order: order.clone(),
            },
        }
        .data(),
    };
    let size = common::template_transport::transaction_size;
    let old_chain_size = size(&staging::transaction(
        &ctx,
        &payer,
        std::slice::from_ref(&direct_chain),
    ));
    let old_prepare_size = size(&staging::transaction(
        &ctx,
        &payer,
        &[direct_announce, create_ata.clone()],
    ));
    assert!(old_chain_size > 1232 && old_prepare_size > 1232);
    let (created, first_size) = staging::send(
        &mut ctx,
        &payer,
        &[staging::init(
            payer.pubkey(),
            seed,
            order.hash(),
            bytes.len(),
            &bytes,
        )],
    );
    created.unwrap();
    let (sealed, second_size) = staging::send(
        &mut ctx,
        &payer,
        &[staging::seal(payer.pubkey(), buffer), create_ata],
    );
    assert!(contains_cpi_event(OrderAnnounced::new(
        order.hash(),
        c.escrow_authority,
        order.clone()
    ))(sealed.unwrap()));
    ctx.intent_chainer().seed_escrow(&c, input);
    let (executed, chain_size) = staging::send(
        &mut ctx,
        &payer,
        &[staging::execute(direct_chain, buffer, false)],
    );
    assert!(contains_event(published(&c, route))(executed.unwrap()));
    assert_eq!(ctx.token_balance(&c.escrow_ata), 0);
    assert_eq!(ctx.token_balance(&c.vault_ata), input);
    assert_eq!((first_size, second_size, chain_size), (1150, 499, 594));
    println!("781-byte Order: old prepare={old_prepare_size}, old chain={old_chain_size}; init={first_size}, seal+ATA={second_size}, chain_from_account={chain_size}");
}

#[test]
fn native_staging_substitution_and_wrong_declared_commitment_move_no_value() {
    let (mut ctx, order, input, route) = setup(&fixtures::nested()["cases"][0]);
    let c = resolve(&mut ctx, &order, input, &route);
    ctx.intent_chainer().seed_escrow(&c, input);
    let authority = ctx.payer.insecure_clone();
    let mut foreign = order.clone();
    foreign.reward.creator = Pubkey::new_unique();
    // A writer can claim the victim commitment but cannot seal different bytes.
    let bad = staging::upload(
        &mut ctx,
        &authority,
        [1; 32],
        order.hash(),
        &borsh::to_vec(&foreign).unwrap(),
    );
    assert!(is_error(ChainerError::OrderCommitmentMismatch)(
        staging::send(
            &mut ctx,
            &authority,
            &[staging::seal(authority.pubkey(), bad)]
        )
        .0
        .unwrap_err()
    ));
    let unsealed = staging::execute(chain_instruction(&ctx, &c), bad, false);
    assert!(is_error(ChainerError::OrderBufferNotSealed)(
        staging::send(&mut ctx, &authority, &[unsealed])
            .0
            .unwrap_err()
    ));
    // Correctly sealed foreign order still cannot authorize the victim escrow.
    let foreign_buffer = staging::upload(
        &mut ctx,
        &authority,
        [2; 32],
        foreign.hash(),
        &borsh::to_vec(&foreign).unwrap(),
    );
    staging::send(
        &mut ctx,
        &authority,
        &[staging::seal(authority.pubkey(), foreign_buffer)],
    )
    .0
    .unwrap();
    let ix = staging::execute(chain_instruction(&ctx, &c), foreign_buffer, false);
    assert!(is_error(ChainerError::InvalidEscrowAuthority)(
        staging::send(&mut ctx, &authority, &[ix]).0.unwrap_err()
    ));
    assert_eq!(ctx.token_balance(&c.escrow_ata), input);
    assert!(ctx.get_account(&c.vault_ata).is_none());
}

#[test]
fn native_staging_write_by_a_execute_by_b_and_separate_rent_reclamation() {
    let (mut ctx, order, input, route) = setup(&fixtures::nested()["cases"][6]);
    let c = resolve(&mut ctx, &order, input, &route);
    ctx.intent_chainer().seed_escrow(&c, input);
    let authority = solana_sdk::signature::Keypair::new();
    ctx.airdrop(&authority.pubkey(), 100_000_000).unwrap();
    let payer = ctx.payer.insecure_clone();
    let buffer = staging::upload(
        &mut ctx,
        &authority,
        [3; 32],
        order.hash(),
        &borsh::to_vec(&order).unwrap(),
    );
    for ix in [
        staging::write(payer.pubkey(), buffer, 0, &[1]),
        staging::seal(payer.pubkey(), buffer),
        staging::close(payer.pubkey(), buffer),
    ] {
        assert!(staging::send(&mut ctx, &payer, &[ix]).0.is_err());
    }
    staging::send(
        &mut ctx,
        &authority,
        &[staging::seal(authority.pubkey(), buffer)],
    )
    .0
    .unwrap();
    let state_before = ctx.get_account(&buffer).unwrap();
    assert!(is_error(ChainerError::OrderBufferSealed)(
        staging::send(
            &mut ctx,
            &authority,
            &[staging::write(authority.pubkey(), buffer, 0, &[1])]
        )
        .0
        .unwrap_err()
    ));
    let ix = staging::execute(chain_instruction(&ctx, &c), buffer, false);
    assert!(!ix.accounts[0].is_writable && !ix.accounts[0].is_signer);
    assert!(!ix.accounts.iter().any(|m| m.pubkey == authority.pubkey()));
    let owner_balance = ctx.get_balance(&authority.pubkey()).unwrap();
    let (result, _) = staging::send(&mut ctx, &payer, std::slice::from_ref(&ix));
    result.unwrap();
    assert_eq!(ctx.get_balance(&authority.pubkey()).unwrap(), owner_balance);
    assert_eq!(ctx.get_account(&buffer).unwrap(), state_before);
    assert_eq!(ctx.token_balance(&c.vault_ata), input);
    assert!(is_error(ChainerError::ZeroAmount)(
        staging::send(&mut ctx, &payer, &[ix]).0.unwrap_err()
    ));
    let mut new_order = order.clone();
    new_order.reward.creator = Pubkey::new_unique();
    let new_escrow = resolve(&mut ctx, &new_order, input, &route);
    ctx.intent_chainer().seed_escrow(&new_escrow, input);
    let replay = staging::execute(chain_instruction(&ctx, &new_escrow), buffer, false);
    assert!(is_error(ChainerError::InvalidEscrowAuthority)(
        staging::send(&mut ctx, &payer, &[replay]).0.unwrap_err()
    ));
    assert_eq!(ctx.token_balance(&new_escrow.escrow_ata), input);
    // Close is a separate transaction and is available after execution failure.
    let close = staging::close(authority.pubkey(), buffer);
    let fee = staging::send(&mut ctx, &authority, &[close]).0.unwrap().fee;
    assert!(ctx.get_account(&buffer).is_none());
    assert_eq!(
        ctx.get_balance(&authority.pubkey()).unwrap(),
        owner_balance + state_before.lamports - fee
    );
}

#[test]
fn native_staging_close_recreate_cannot_redirect_a_pending_execution() {
    let (mut ctx, order, input, route) = setup(&fixtures::nested()["cases"][0]);
    let c = resolve(&mut ctx, &order, input, &route);
    ctx.intent_chainer().seed_escrow(&c, input);
    let payer = ctx.payer.insecure_clone();
    let bytes = borsh::to_vec(&order).unwrap();
    let seed = [4; 32];
    let buffer = staging::upload(&mut ctx, &payer, seed, order.hash(), &bytes);
    staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
        .0
        .unwrap();
    let pending = staging::execute(chain_instruction(&ctx, &c), buffer, false);
    staging::send(&mut ctx, &payer, &[staging::close(payer.pubkey(), buffer)])
        .0
        .unwrap();
    assert!(
        staging::send(&mut ctx, &payer, std::slice::from_ref(&pending))
            .0
            .is_err()
    );
    let mut substitute = order.clone();
    substitute.reward.creator = Pubkey::new_unique();
    assert_eq!(
        staging::upload(
            &mut ctx,
            &payer,
            seed,
            substitute.hash(),
            &borsh::to_vec(&substitute).unwrap()
        ),
        buffer
    );
    staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
        .0
        .unwrap();
    assert!(is_error(ChainerError::InvalidEscrowAuthority)(
        staging::send(&mut ctx, &payer, std::slice::from_ref(&pending))
            .0
            .unwrap_err()
    ));
    assert_eq!(ctx.token_balance(&c.escrow_ata), input);
    assert!(ctx.get_account(&c.vault_ata).is_none());
    // Re-staging identical bytes restores liveness, even after its deadline.
    staging::send(&mut ctx, &payer, &[staging::close(payer.pubkey(), buffer)])
        .0
        .unwrap();
    staging::upload(&mut ctx, &payer, seed, order.hash(), &bytes);
    ctx.warp_to_timestamp((order.reward.deadline + 1).try_into().unwrap());
    staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
        .0
        .unwrap();
    staging::send(&mut ctx, &payer, &[pending]).0.unwrap();
    assert_eq!(ctx.token_balance(&c.vault_ata), input);
}

#[test]
fn native_staging_maximum_nested_orders_hold_under_stock_heap_and_publish_fully() {
    let (mut ctx, base, _, _) = setup(&fixtures::nested()["cases"][6]);
    let (dense, dense_root) = maximum_order(&base);
    let mut large_root = base;
    let mut node = large_root.template.vaults[0].clone();
    node.route = Template::literal(vec![1]);
    node.reward = Template::literal(vec![2]);
    large_root.template.vaults = vec![node; MAX_VAULTS];
    let root = vec![0xa5; MAX_RENDERED_BYTES - 2 * MAX_VAULTS];
    large_root.template.route = Template::literal(root.clone());
    let payer = ctx.payer.insecure_clone();
    for (order, route) in [(dense, dense_root), (large_root, root)] {
        let c = resolve(&mut ctx, &order, 1, &route);
        let bytes = borsh::to_vec(&order).unwrap();
        let buffer = staging::upload(
            &mut ctx,
            &payer,
            Pubkey::new_unique().to_bytes(),
            order.hash(),
            &bytes,
        );
        let announcement =
            staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
                .0
                .unwrap();
        assert!(contains_cpi_event(OrderAnnounced::new(
            order.hash(),
            c.escrow_authority,
            order.clone()
        ))(announcement));
        ctx.intent_chainer().seed_escrow(&c, 1);
        let ix = staging::execute(chain_instruction(&ctx, &c), buffer, false);
        let result = staging::send(&mut ctx, &payer, &[ix]).0.unwrap();
        assert!(contains_event(published(&c, route.clone()))(result.clone()));
        assert_eq!(ctx.token_balance(&c.escrow_ata), 0);
        assert_eq!(ctx.token_balance(&c.vault_ata), 1);
        println!(
            "native maximum: Order={} root={} chain CU={}",
            bytes.len(),
            route.len(),
            result.compute_units_consumed
        );
    }
}

#[test]
fn native_staging_bounds_append_only_and_abandoned_rent() {
    let (mut ctx, order, _, _) = setup(&fixtures::nested()["cases"][0]);
    let payer = ctx.payer.insecure_clone();
    let seed = [5; 32];
    let buffer = OrderBuffer::pda(&payer.pubkey(), &seed).0;
    for (len, bytes) in [
        (0, vec![]),
        (MAX_ORDER_BYTES + 1, vec![]),
        (900, vec![0; 801]),
    ] {
        assert!(staging::send(
            &mut ctx,
            &payer,
            &[staging::init(
                payer.pubkey(),
                seed,
                order.hash(),
                len,
                &bytes
            )]
        )
        .0
        .is_err());
        assert!(ctx.get_account(&buffer).is_none());
    }
    // Stray lamports cannot grief initialization. The original funder alone
    // reclaims both rent and donations on the separate close instruction.
    let prefund = ctx
        .get_sysvar::<solana_sdk::rent::Rent>()
        .minimum_balance(0);
    ctx.airdrop(&buffer, prefund).unwrap();
    let bytes = borsh::to_vec(&order).unwrap();
    let init = staging::init(
        payer.pubkey(),
        seed,
        order.hash(),
        bytes.len(),
        &bytes[..100],
    );
    staging::send(&mut ctx, &payer, std::slice::from_ref(&init))
        .0
        .unwrap();
    assert!(staging::send(&mut ctx, &payer, &[init]).0.is_err());
    let before = ctx.get_account(&buffer).unwrap();
    for (offset, bytes) in [
        (0, vec![1]),
        (101, vec![1]),
        (u32::MAX, vec![1]),
        (100, vec![]),
        (100, vec![0; 801]),
    ] {
        assert!(is_error(ChainerError::InvalidOrderBufferWrite)(
            staging::send(
                &mut ctx,
                &payer,
                &[staging::write(payer.pubkey(), buffer, offset, &bytes)]
            )
            .0
            .unwrap_err()
        ));
        assert_eq!(ctx.get_account(&buffer).unwrap(), before);
    }
    assert!(is_error(ChainerError::OrderBufferIncomplete)(
        staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
            .0
            .unwrap_err()
    ));
    let before_balance = ctx.get_balance(&payer.pubkey()).unwrap();
    let fee = staging::send(&mut ctx, &payer, &[staging::close(payer.pubkey(), buffer)])
        .0
        .unwrap()
        .fee;
    assert!(ctx.get_account(&buffer).is_none());
    assert_eq!(
        ctx.get_balance(&payer.pubkey()).unwrap(),
        before_balance + before.lamports - fee
    );
}

#[test]
fn native_staging_exact_decode_and_validation_parity_even_with_a_forged_sealed_header() {
    use anchor_lang::AccountSerialize;
    for invalid in 0..8 {
        let (mut ctx, mut order, input, route) = setup(&fixtures::nested()["cases"][0]);
        let c = resolve(&mut ctx, &order, input, &route);
        let expected = match invalid {
            0 => {
                order.scale = 0;
                ChainerError::InvalidScale
            }
            1 => {
                order.template.route.segments.pop();
                ChainerError::SegmentCountMismatch
            }
            2 => {
                order.template.vaults[0].reward.items[0] = Item::Vault(0);
                ChainerError::InvalidVaultReference
            }
            3 => {
                if let Derivation::Evm(config) = &mut order.template.vaults[0].derivation {
                    config.portal = [0; 20];
                }
                ChainerError::MissingRemotePortal
            }
            7 => ChainerError::OrderCommitmentMismatch,
            _ => ChainerError::InvalidBufferedOrder,
        };
        ctx.intent_chainer().seed_escrow(&c, input);
        let payer = ctx.payer.insecure_clone();
        let mut bytes = borsh::to_vec(&order).unwrap();
        match invalid {
            4 => bytes.push(0), // trailing bytes must not be silently ignored
            5 => {
                bytes.pop();
            }
            6 => bytes[72..76].copy_from_slice(&u32::MAX.to_le_bytes()),
            _ => {}
        }
        let commitment = if invalid == 7 {
            [0xff; 32].into()
        } else {
            order.hash()
        };
        let buffer = staging::upload(&mut ctx, &payer, [6; 32], commitment, &bytes);
        let error = staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
            .0
            .unwrap_err();
        let expected_code = u32::from(expected);
        let code = |e: &litesvm::types::FailedTransactionMetadata| match &e.err {
            solana_sdk::transaction::TransactionError::InstructionError(
                _,
                solana_sdk::instruction::InstructionError::Custom(code),
            ) => *code,
            other => panic!("unexpected error {other:?}"),
        };
        assert_eq!(code(&error), expected_code);
        let original = ctx.get_account(&buffer).unwrap();
        let mut header = OrderBuffer::try_deserialize(&mut &original.data[..]).unwrap();
        assert!(!header.sealed);
        // Test-only state injection. No public instruction can create this state:
        // prove execute revalidates instead of trusting the earlier seal check.
        header.sealed = true;
        let mut forged = original;
        header
            .try_serialize(&mut &mut forged.data[..OrderBuffer::HEADER_LEN])
            .unwrap();
        ctx.set_account(buffer, forged).unwrap();
        let ix = staging::execute(chain_instruction(&ctx, &c), buffer, false);
        assert_eq!(
            code(&staging::send(&mut ctx, &payer, &[ix]).0.unwrap_err()),
            expected_code
        );
        assert_eq!(ctx.token_balance(&c.escrow_ata), input);
        assert!(ctx.get_account(&c.vault_ata).is_none());
        staging::send(&mut ctx, &payer, &[staging::close(payer.pubkey(), buffer)])
            .0
            .unwrap();
    }
}

#[test]
fn native_staging_stale_accounts_fail_atomically_and_retry_remeasures() {
    let (mut ctx, order, input, route) = setup(&fixtures::nested()["cases"][0]);
    let c = resolve(&mut ctx, &order, input, &route);
    ctx.intent_chainer().seed_escrow(&c, input + 1);
    let payer = ctx.payer.insecure_clone();
    let buffer = staging::upload(
        &mut ctx,
        &payer,
        [7; 32],
        order.hash(),
        &borsh::to_vec(&order).unwrap(),
    );
    staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
        .0
        .unwrap();
    let before = ctx.get_account(&buffer).unwrap();
    let ix = staging::execute(chain_instruction(&ctx, &c), buffer, false);
    assert!(is_error(ChainerError::InvalidVault)(
        staging::send(&mut ctx, &payer, &[ix]).0.unwrap_err()
    ));
    assert_eq!(ctx.token_balance(&c.escrow_ata), input + 1);
    assert!(ctx.get_account(&c.vault_ata).is_none());
    assert_eq!(ctx.get_account(&buffer).unwrap(), before);
    let updated_route = order.build_route(input + 1).unwrap();
    let updated = resolve(&mut ctx, &order, input + 1, &updated_route);
    let ix = staging::execute(chain_instruction(&ctx, &updated), buffer, false);
    assert!(contains_event(published(&updated, updated_route))(
        staging::send(&mut ctx, &payer, &[ix]).0.unwrap()
    ));
    assert_eq!(ctx.token_balance(&updated.vault_ata), input + 1);
}

#[test]
fn native_staging_rejects_foreign_owner_wrong_pda_and_wrong_discriminator() {
    let (mut ctx, order, input, route) = setup(&fixtures::nested()["cases"][6]);
    let c = resolve(&mut ctx, &order, input, &route);
    ctx.intent_chainer().seed_escrow(&c, input);
    let payer = ctx.payer.insecure_clone();
    let buffer = staging::upload(
        &mut ctx,
        &payer,
        [9; 32],
        order.hash(),
        &borsh::to_vec(&order).unwrap(),
    );
    staging::send(&mut ctx, &payer, &[staging::seal(payer.pubkey(), buffer)])
        .0
        .unwrap();
    let original = ctx.get_account(&buffer).unwrap();
    for invalid in 0..3 {
        let mut account = original.clone();
        let address = match invalid {
            0 => {
                account.owner = portal::ID;
                buffer
            }
            1 => Pubkey::new_unique(),
            _ => {
                account.data[0] ^= 1;
                buffer
            }
        };
        ctx.set_account(address, account).unwrap();
        let ix = staging::execute(chain_instruction(&ctx, &c), address, false);
        let error = staging::send(&mut ctx, &payer, &[ix]).0.unwrap_err();
        let expected = match invalid {
            0 => anchor_lang::error::ErrorCode::AccountOwnedByWrongProgram,
            1 => anchor_lang::error::ErrorCode::ConstraintSeeds,
            _ => anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch,
        };
        assert!(is_error(expected)(error));
        assert_eq!(ctx.token_balance(&c.escrow_ata), input);
        assert!(ctx.get_account(&c.vault_ata).is_none());
        ctx.set_account(buffer, original.clone()).unwrap();
    }
}
