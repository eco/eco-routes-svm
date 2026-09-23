//! A source route can commit to publishing its recovery preimage before it
//! delivers funds. This tests the actual Flash -> Portal -> Chainer -> Chainer
//! stack with SBF programs. An SPL transfer stands in for Jupiter's output;
//! this is not a live Jupiter integration or a claim about all swap routes.

mod common;

use anchor_lang::prelude::AccountMeta;
use anchor_lang::{Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use common::template_transport::{transaction_size, PACKET_BYTES};
use common::{contains_cpi_event, is_error, Context};
use eco_svm_std::prover::Proof;
use eco_svm_std::{event_authority_pda, Bytes32, CHAIN_ID};
use flash_fulfiller::instructions::{
    AppendFlashFulfillIntentChunkArgs, FlashFulfillArgs, FlashFulfillIntent,
};
use flash_fulfiller::state::{flash_vault_pda, prove_authority_pda, FlashFulfillIntentAccount};
use intent_chainer::events::OrderAnnounced;
use intent_chainer::instructions::{AnnounceOrderArgs, ChainerError};
use intent_chainer::state::escrow_authority_pda;
use intent_chainer::types::{Order, Template, WAD};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use portal::instructions::PortalError;
use portal::state::{executor_pda, proof_closer_pda, vault_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{
    intent_hash, Call, Calldata, CalldataWithAccounts, Reward, Route, TokenAmount,
};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

const DELIVERED: u64 = 1_234_567;
const FLASH_HEAP: u32 = 256 * 1024;

struct Source {
    order: Order,
    route: Route,
    reward: Reward,
    hash: Bytes32,
    escrow: Pubkey,
    source_vault_ata: Pubkey,
    calls: Vec<AccountMeta>,
}

fn announcement_call(order: &Order) -> (Call, Vec<AccountMeta>) {
    let accounts = intent_chainer::accounts::AnnounceOrder {
        event_authority: event_authority_pda(&intent_chainer::ID).0,
        program: intent_chainer::ID,
    }
    .to_account_metas(None);
    let calldata = Calldata {
        data: intent_chainer::instruction::AnnounceOrder {
            args: AnnounceOrderArgs {
                order: order.clone(),
            },
        }
        .data(),
        account_count: accounts.len() as u8,
    };
    (
        Call {
            target: intent_chainer::ID.to_bytes().into(),
            data: borsh::to_vec(&CalldataWithAccounts::new(calldata, accounts.clone()).unwrap())
                .unwrap(),
        },
        accounts,
    )
}

fn setup(large: bool, invalid: bool) -> (Context, Source) {
    let mut ctx = Context::default();
    let mint = Pubkey::new_unique();
    ctx.set_mint_account(&mint);
    let mut order =
        ctx.intent_chainer()
            .svm_order(mint, Pubkey::new_unique(), local_prover::ID, WAD, 1);
    if large {
        // A valid, opaque downstream route with a large application payload.
        // It stresses the source route's inline announcement and buffered
        // Flash fulfillment without depending on an external swap program.
        let child = Route {
            deadline: ctx.now() + 1800,
            salt: [0x68; 32].into(),
            portal: portal::ID.to_bytes().into(),
            native_amount: 0,
            tokens: vec![],
            calls: vec![Call {
                target: common::SPL_NOOP_ID.to_bytes().into(),
                data: vec![0x45; 1536],
            }],
        };
        order.template.route = Template::literal(borsh::to_vec(&child).unwrap());
        assert!(borsh::to_vec(&order).unwrap().len() > 1800);
    }
    if invalid {
        // Announcement validates the canonical reward, before the delivery call.
        order.reward.tokens[0].amount = 1;
    }
    let authority = escrow_authority_pda(&order.hash()).0;
    ctx.airdrop_token_ata(&mint, &authority, 0);
    let escrow =
        get_associated_token_address_with_program_id(&authority, &mint, &ctx.token_program);
    let (announce, announce_accounts) = announcement_call(&order);
    let (_, mut deliver, mut deliver_accounts) = ctx
        .intent_chainer()
        .deliver_to_escrow_call(mint, escrow, DELIVERED);
    // Flash's fulfill CPI supplies the executor as writable. Route commitments
    // must use that effective privilege, as the existing Flash SPL-call tests
    // do; the shared delivery helper defaults to a direct Portal invocation.
    deliver_accounts[3] = AccountMeta::new(executor_pda().0, false);
    let full: CalldataWithAccounts = borsh::from_slice(&deliver.data).unwrap();
    deliver.data =
        borsh::to_vec(&CalldataWithAccounts::new(full.calldata, deliver_accounts.clone()).unwrap())
            .unwrap();
    let route = Route {
        deadline: ctx.now() + 1800,
        salt: [0x69; 32].into(),
        portal: portal::ID.to_bytes().into(),
        native_amount: 0,
        tokens: vec![TokenAmount {
            token: mint,
            amount: DELIVERED,
        }],
        calls: vec![announce, deliver],
    };
    let reward = Reward {
        deadline: ctx.now() + 3600,
        creator: ctx.creator.pubkey(),
        prover: local_prover::ID,
        native_amount: 0,
        tokens: route.tokens.clone(),
    };
    let hash = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let vault = vault_pda(&hash).0;
    let funder = ctx.funder.pubkey();
    ctx.airdrop_token_ata(&mint, &funder, DELIVERED);
    let source_vault_ata =
        get_associated_token_address_with_program_id(&vault, &mint, &ctx.token_program);
    let funder_ata =
        get_associated_token_address_with_program_id(&funder, &mint, &ctx.token_program);
    ctx.portal()
        .fund_intent(
            CHAIN_ID,
            reward.clone(),
            vault,
            route.hash(),
            false,
            vec![
                AccountMeta::new(funder_ata, false),
                AccountMeta::new(source_vault_ata, false),
                AccountMeta::new_readonly(mint, false),
            ],
        )
        .unwrap();
    let calls = [announce_accounts, deliver_accounts].concat();
    (
        ctx,
        Source {
            order,
            route,
            reward,
            hash,
            escrow,
            source_vault_ata,
            calls,
        },
    )
}

/// Use the production Flash intent buffer, never the generic test-only staging
/// program. Check every submitted packet, including writes and final execution.
fn stage_source(ctx: &mut Context, source: &Source) -> Pubkey {
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&writer.pubkey(), &source.hash).0;
    let mut payload = FlashFulfillIntentAccount::DISCRIMINATOR.to_vec();
    payload.extend_from_slice(&borsh::to_vec(&source.route).unwrap());
    payload.extend_from_slice(&borsh::to_vec(&source.reward).unwrap());
    for chunk in payload.chunks(600) {
        let ix = Instruction {
            program_id: flash_fulfiller::ID,
            accounts: flash_fulfiller::accounts::AppendFlashFulfillIntentChunk {
                writer: writer.pubkey(),
                flash_fulfill_intent: buffer,
                system_program: anchor_lang::system_program::ID,
            }
            .to_account_metas(None),
            data: flash_fulfiller::instruction::AppendFlashFulfillIntentChunk {
                args: AppendFlashFulfillIntentChunkArgs {
                    intent_hash: source.hash,
                    chunk: chunk.to_vec(),
                },
            }
            .data(),
        };
        let tx = Transaction::new(
            &[&writer],
            Message::new(
                &[ComputeBudgetInstruction::request_heap_frame(FLASH_HEAP), ix],
                Some(&writer.pubkey()),
            ),
            ctx.latest_blockhash(),
        );
        assert!(transaction_size(&tx) <= PACKET_BYTES);
        ctx.send_transaction(tx)
            .expect("native source buffer write must succeed");
    }
    buffer
}

fn fulfill_source(
    ctx: &mut Context,
    source: &Source,
) -> Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
    let buffer = stage_source(ctx, source);
    let payer = ctx.payer.insecure_clone();
    let mint = source.order.base_mint;
    let ata =
        |owner| get_associated_token_address_with_program_id(&owner, &mint, &ctx.token_program);
    let flash_ata = ata(flash_vault_pda().0);
    let executor_ata = ata(executor_pda().0);
    let claimant_ata = ata(payer.pubkey());
    ctx.airdrop_token_ata(&mint, &payer.pubkey(), 0);
    let mut accounts = flash_fulfiller::accounts::FlashFulfill {
        payer: payer.pubkey(),
        flash_vault: flash_vault_pda().0,
        flash_fulfill_intent: Some(buffer),
        writer: Some(payer.pubkey()),
        claimant: payer.pubkey(),
        proof: Proof::pda(&source.hash, &local_prover::ID).0,
        intent_vault: vault_pda(&source.hash).0,
        withdrawn_marker: WithdrawnMarker::pda(&source.hash).0,
        proof_closer: proof_closer_pda(&local_prover::ID).0,
        executor: executor_pda().0,
        fulfill_marker: FulfillMarker::pda(&source.hash).0,
        portal_program: portal::ID,
        local_prover_program: local_prover::ID,
        prove_authority: prove_authority_pda(&local_prover::ID).0,
        local_prover_event_authority: event_authority_pda(&local_prover::ID).0,
        token_program: anchor_spl::token::ID,
        token_2022_program: anchor_spl::token_2022::ID,
        associated_token_program: anchor_spl::associated_token::ID,
        system_program: anchor_lang::system_program::ID,
        event_authority: event_authority_pda(&flash_fulfiller::ID).0,
        program: flash_fulfiller::ID,
    }
    .to_account_metas(None);
    accounts.extend([
        AccountMeta::new(source.source_vault_ata, false),
        AccountMeta::new(flash_ata, false),
        AccountMeta::new_readonly(mint, false),
        AccountMeta::new(flash_ata, false),
        AccountMeta::new(executor_ata, false),
        AccountMeta::new_readonly(mint, false),
        AccountMeta::new(claimant_ata, false),
    ]);
    accounts.extend(source.calls.clone());
    let ix = Instruction {
        program_id: flash_fulfiller::ID,
        accounts,
        data: flash_fulfiller::instruction::FlashFulfill {
            args: FlashFulfillArgs {
                intent: FlashFulfillIntent::IntentHash(source.hash),
            },
        }
        .data(),
    };
    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[
                ComputeBudgetInstruction::set_compute_unit_limit(1_000_000),
                ComputeBudgetInstruction::request_heap_frame(FLASH_HEAP),
                ix,
            ],
            Some(&payer.pubkey()),
        ),
        ctx.latest_blockhash(),
    );
    let size = transaction_size(&tx);
    assert!(
        size <= PACKET_BYTES,
        "source fulfill packet is {size} bytes"
    );
    let result = ctx.send_transaction(tx);
    if let Ok(meta) = &result {
        println!(
            "announced source: order={} fulfill_packet={} CU={}",
            borsh::to_vec(&source.order).unwrap().len(),
            size,
            meta.compute_units_consumed
        );
    }
    result
}

#[test]
fn flash_source_records_full_order_before_funding_escrow() {
    for large in [false, true] {
        let (mut ctx, source) = setup(large, false);
        let meta = fulfill_source(&mut ctx, &source)
            .expect("committed announcement must execute through the Flash stack");
        assert!(contains_cpi_event(OrderAnnounced::new(
            source.order.hash(),
            escrow_authority_pda(&source.order.hash()).0,
            source.order.clone()
        ))(meta.clone()));
        // Pin the real call stack, not merely a top-level announcement event.
        assert!(meta
            .logs
            .iter()
            .any(|line| line == &format!("Program {} invoke [4]", intent_chainer::ID)));
        assert_eq!(ctx.token_balance(&source.escrow), DELIVERED);
        assert_eq!(ctx.token_balance(&source.source_vault_ata), 0);
        assert!(ctx
            .account::<FulfillMarker>(&FulfillMarker::pda(&source.hash).0)
            .is_some());
        assert!(ctx
            .account::<WithdrawnMarker>(&WithdrawnMarker::pda(&source.hash).0)
            .is_some());
        // The source transport can disappear: the full recovery order remains
        // in the successful transaction's durable self-CPI instruction.
        assert!(ctx
            .get_account(&FlashFulfillIntentAccount::pda(&ctx.payer.pubkey(), &source.hash).0)
            .is_none());
    }
}

#[test]
fn invalid_announcement_rolls_back_source_withdrawal_and_delivery() {
    let (mut ctx, source) = setup(false, true);
    let error = fulfill_source(&mut ctx, &source).unwrap_err();
    assert!(error
        .meta
        .logs
        .iter()
        .any(|line| line.contains("Instruction: AnnounceOrder")));
    assert!(is_error(ChainerError::RewardAmountMustBeZero)(error));
    assert_eq!(ctx.token_balance(&source.source_vault_ata), DELIVERED);
    assert_eq!(ctx.token_balance(&source.escrow), 0);
    assert!(ctx
        .account::<WithdrawnMarker>(&WithdrawnMarker::pda(&source.hash).0)
        .is_none());
    assert!(ctx
        .account::<FulfillMarker>(&FulfillMarker::pda(&source.hash).0)
        .is_none());
}

#[test]
fn fulfiller_cannot_omit_or_replace_the_committed_announcement() {
    for omit in [false, true] {
        let (mut ctx, source) = setup(false, false);
        let mut route = source.route.clone();
        let mut calls = source.calls.clone();
        if omit {
            route.calls.remove(0);
            calls.drain(..2);
        } else {
            let mut substitute = source.order.clone();
            substitute.reward.creator = Pubkey::new_unique();
            route.calls[0] = announcement_call(&substitute).0;
        }
        // Invoke Portal directly as an independent fulfiller would. It executes
        // the supplied route then checks its reconstructed hash against the
        // original funded intent, so any delivery must roll back on mismatch.
        for call in &mut route.calls {
            let full: CalldataWithAccounts = borsh::from_slice(&call.data).unwrap();
            call.data = borsh::to_vec(&full.calldata).unwrap();
        }
        let mint = source.order.base_mint;
        let solver = ctx.solver.pubkey();
        ctx.airdrop_token_ata(&mint, &solver, DELIVERED);
        let solver_ata =
            get_associated_token_address_with_program_id(&solver, &mint, &ctx.token_program);
        let executor_ata = get_associated_token_address_with_program_id(
            &executor_pda().0,
            &mint,
            &ctx.token_program,
        );
        if omit {
            calls.push(AccountMeta::new_readonly(intent_chainer::ID, false));
        }
        let error = ctx
            .portal()
            .fulfill_intent(
                source.hash,
                &route,
                source.reward.hash(),
                solver.to_bytes().into(),
                executor_pda().0,
                FulfillMarker::pda(&source.hash).0,
                vec![
                    AccountMeta::new(solver_ata, false),
                    AccountMeta::new(executor_ata, false),
                    AccountMeta::new_readonly(mint, false),
                ],
                calls,
            )
            .unwrap_err();
        assert!(is_error(PortalError::InvalidIntentHash)(error));
        assert_eq!(ctx.token_balance(&solver_ata), DELIVERED);
        assert_eq!(ctx.token_balance(&source.source_vault_ata), DELIVERED);
        assert_eq!(ctx.token_balance(&source.escrow), 0);
        assert!(ctx
            .account::<FulfillMarker>(&FulfillMarker::pda(&source.hash).0)
            .is_none());
    }
}
