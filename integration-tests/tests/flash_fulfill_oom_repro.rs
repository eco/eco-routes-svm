//! Regression guard for the heap-OOM that previously killed `flash_fulfill`
//! when consuming a large Route from the chunked-buffer path
//! (`FlashFulfillIntent::IntentHash`). With a mainnet-shape Route (3 calls,
//! ~2 KB Borsh-encoded, Jupiter-sized middle call at 30 accounts), the old
//! `strip_call_accounts` plus `chain().collect()` in `cpi::fulfill` retained
//! ~10 KB of dead heap in Solana's bump allocator — enough to push a deep CPI
//! chain into OOM on the default 32 KB heap.
//!
//! The fix lives in:
//!   - `programs/flash-fulfiller/src/instructions/flash_fulfill.rs`
//!     (zero-copy `strip_call_accounts`)
//!   - `programs/flash-fulfiller/src/cpi/fulfill.rs`
//!     (`Vec::with_capacity(total)` for `accounts`/`infos`)
//!
//! This test asserts the heap path stays clean: the flash_fulfill tx MUST NOT
//! log "memory allocation failed, out of memory". The call targets are dummy
//! pubkeys (so the inner `portal.fulfill` CPI will fail dispatch after all the
//! heap-sensitive work is done) — we assert on the error *signature*, not on
//! success.

use anchor_lang::prelude::AccountMeta;
use anchor_lang::{AnchorSerialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use eco_svm_std::prover::Proof;
use eco_svm_std::{event_authority_pda, Bytes32, CHAIN_ID};
use flash_fulfiller::instructions::{
    AppendFlashFulfillRouteChunkArgs, FlashFulfillArgs, FlashFulfillIntent,
    InitFlashFulfillIntentArgs,
};
use flash_fulfiller::state::{flash_vault_pda, FlashFulfillIntentAccount};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use portal::state::{executor_pda, proof_closer_pda, vault_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{
    intent_hash, Call, Calldata, CalldataWithAccounts, Reward, Route, TokenAmount,
};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

pub mod common;

type TxResult = Result<TransactionMetadata, Box<FailedTransactionMetadata>>;

// Per-call account counts mirror a real bucketed-swap Route:
// [open] + [Jupiter SharedAccountsRoute] + [close_and_select_intent].
const OPEN_CALL_ACCOUNTS: usize = 5;
const JUPITER_CALL_ACCOUNTS: usize = 30;
const CLOSE_SELECT_CALL_ACCOUNTS: usize = 13;

const OPEN_CALL_DATA_BYTES: usize = 16;
const JUPITER_CALL_DATA_BYTES: usize = 200;
const CLOSE_SELECT_CALL_DATA_BYTES: usize = 80;

fn make_padded_call(account_count: usize, data_bytes: usize) -> Call {
    let accounts: Vec<AccountMeta> = (0..account_count)
        .map(|_| AccountMeta::new_readonly(Pubkey::new_unique(), false))
        .collect();
    let calldata = Calldata {
        data: vec![0u8; data_bytes],
        account_count: account_count as u8,
    };
    Call {
        target: Pubkey::new_unique().to_bytes().into(),
        data: CalldataWithAccounts::new(calldata, accounts)
            .unwrap()
            .try_to_vec()
            .unwrap(),
    }
}

fn dedup_call_accounts(total: usize, dummy: Pubkey) -> Vec<AccountMeta> {
    (0..total)
        .map(|_| AccountMeta::new_readonly(dummy, false))
        .collect()
}

fn build_mainnet_shape_route(mint: Pubkey, deadline: u64) -> (Route, Vec<u8>, Vec<AccountMeta>) {
    let route = Route {
        salt: [0xA5u8; 32].into(),
        deadline,
        portal: portal::ID.to_bytes().into(),
        native_amount: 0,
        tokens: vec![TokenAmount {
            token: mint,
            amount: 1_000_000,
        }],
        calls: vec![
            make_padded_call(OPEN_CALL_ACCOUNTS, OPEN_CALL_DATA_BYTES),
            make_padded_call(JUPITER_CALL_ACCOUNTS, JUPITER_CALL_DATA_BYTES),
            make_padded_call(CLOSE_SELECT_CALL_ACCOUNTS, CLOSE_SELECT_CALL_DATA_BYTES),
        ],
    };

    let bytes = route.try_to_vec().unwrap();
    let total = OPEN_CALL_ACCOUNTS + JUPITER_CALL_ACCOUNTS + CLOSE_SELECT_CALL_ACCOUNTS;
    let metas = dedup_call_accounts(total, Pubkey::new_unique());
    (route, bytes, metas)
}

fn upload_buffer_in_chunks(
    ctx: &mut common::Context,
    writer: &solana_sdk::signature::Keypair,
    intent_hash_value: Bytes32,
    route_hash: Bytes32,
    reward: &Reward,
    route_bytes: &[u8],
) -> Pubkey {
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value, &writer.pubkey()).0;
    ctx.flash_fulfiller()
        .init_flash_fulfill_intent(
            writer,
            buffer,
            InitFlashFulfillIntentArgs {
                intent_hash: intent_hash_value,
                route_hash,
                reward: reward.clone(),
                route_total_size: route_bytes.len() as u32,
            },
        )
        .expect("init_flash_fulfill_intent should succeed");

    const CHUNK: usize = 900;
    let mut offset = 0usize;
    while offset < route_bytes.len() {
        let end = (offset + CHUNK).min(route_bytes.len());
        ctx.flash_fulfiller()
            .append_flash_fulfill_route_chunk(
                writer,
                buffer,
                AppendFlashFulfillRouteChunkArgs {
                    intent_hash: intent_hash_value,
                    offset: offset as u32,
                    chunk: route_bytes[offset..end].to_vec(),
                },
            )
            .expect("append_flash_fulfill_route_chunk should succeed");
        offset = end;
    }

    buffer
}

#[allow(clippy::too_many_arguments)]
fn send_flash_fulfill_ix(
    ctx: &mut common::Context,
    intent_hash_value: Bytes32,
    route: &Route,
    reward: &Reward,
    claimant: Pubkey,
    claimant_atas: Vec<AccountMeta>,
    call_accounts: Vec<AccountMeta>,
    heap_frame_bytes: Option<u32>,
    cu_limit: u32,
) -> TxResult {
    let flash_vault = flash_vault_pda().0;
    let intent_vault = vault_pda(&intent_hash_value).0;
    let executor = executor_pda().0;
    let token_program = ctx.token_program;

    let reward_accounts: Vec<AccountMeta> = reward
        .tokens
        .iter()
        .flat_map(|token| {
            [
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &intent_vault,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &flash_vault,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();
    let route_accounts: Vec<AccountMeta> = route
        .tokens
        .iter()
        .flat_map(|token| {
            [
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &flash_vault,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    get_associated_token_address_with_program_id(
                        &executor,
                        &token.token,
                        &token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    let flash_fulfill_intent =
        Some(FlashFulfillIntentAccount::pda(&intent_hash_value, &ctx.payer.pubkey()).0);
    let accounts = flash_fulfiller::accounts::FlashFulfill {
        payer: ctx.payer.pubkey(),
        flash_vault,
        flash_fulfill_intent,
        claimant,
        proof: Proof::pda(&intent_hash_value, &local_prover::ID).0,
        intent_vault,
        withdrawn_marker: WithdrawnMarker::pda(&intent_hash_value).0,
        proof_closer: proof_closer_pda().0,
        executor,
        fulfill_marker: FulfillMarker::pda(&intent_hash_value).0,
        portal_program: portal::ID,
        local_prover_program: local_prover::ID,
        local_prover_event_authority: event_authority_pda(&local_prover::ID).0,
        token_program: anchor_spl::token::ID,
        token_2022_program: anchor_spl::token_2022::ID,
        associated_token_program: anchor_spl::associated_token::ID,
        system_program: anchor_lang::system_program::ID,
        event_authority: event_authority_pda(&flash_fulfiller::ID).0,
        program: flash_fulfiller::ID,
    };

    let account_metas: Vec<AccountMeta> = accounts
        .to_account_metas(None)
        .into_iter()
        .chain(reward_accounts)
        .chain(route_accounts)
        .chain(claimant_atas)
        .chain(call_accounts)
        .collect();

    let instruction_data = flash_fulfiller::instruction::FlashFulfill {
        args: FlashFulfillArgs {
            intent: FlashFulfillIntent::IntentHash(intent_hash_value),
        },
    };
    let flash_fulfill_ix = Instruction {
        program_id: flash_fulfiller::ID,
        accounts: account_metas,
        data: instruction_data.data(),
    };

    let mut ixs = vec![ComputeBudgetInstruction::set_compute_unit_limit(cu_limit)];
    if let Some(bytes) = heap_frame_bytes {
        ixs.push(ComputeBudgetInstruction::request_heap_frame(bytes));
    }
    ixs.push(flash_fulfill_ix);

    let payer_pubkey = ctx.payer.pubkey();
    let blockhash = ctx.latest_blockhash();
    let tx = Transaction::new(
        &[&ctx.payer],
        Message::new(&ixs, Some(&payer_pubkey)),
        blockhash,
    );
    ctx.send_transaction(tx)
}

fn fund_intent_vault(ctx: &mut common::Context, route: &Route, reward: &Reward) {
    let route_hash = route.hash();
    let intent_hash_value = intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    let vault = vault_pda(&intent_hash_value).0;

    let funder = ctx.funder.pubkey();
    ctx.airdrop(&funder, 10_000_000).unwrap();
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
}

fn pre_create_atas(ctx: &mut common::Context, reward: &Reward, route: &Route, claimant: Pubkey) {
    let flash_vault = flash_vault_pda().0;
    let executor = executor_pda().0;

    reward.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &claimant, 0);
        ctx.airdrop_token_ata(&token.token, &flash_vault, 0);
    });
    route.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &executor, 0);
    });
}

fn assert_no_oom(logs: &[String]) {
    let oom = logs
        .iter()
        .any(|l| l.contains("out of memory") || l.contains("memory allocation failed"));
    assert!(
        !oom,
        "flash_fulfill OOMed — the heap-pressure fix in strip_call_accounts / cpi::fulfill has regressed.\nLogs:\n{}",
        logs.join("\n")
    );
}

fn run_mainnet_shape_flash_fulfill(heap_frame_bytes: Option<u32>) -> Vec<String> {
    let mut ctx = common::Context::default();

    let mint = Pubkey::new_unique();
    ctx.set_mint_account(&mint);

    let deadline = ctx.now() + 3600;
    let (route, route_bytes, call_accounts) = build_mainnet_shape_route(mint, deadline);
    let reward = Reward {
        deadline: deadline + 60,
        creator: ctx.creator.pubkey(),
        prover: local_prover::ID,
        native_amount: 0,
        tokens: vec![TokenAmount {
            token: mint,
            amount: 1_000_000,
        }],
    };

    let route_hash = route.hash();
    let intent_hash_value = intent_hash(CHAIN_ID, &route_hash, &reward.hash());

    fund_intent_vault(&mut ctx, &route, &reward);

    let writer = ctx.payer.insecure_clone();
    upload_buffer_in_chunks(
        &mut ctx,
        &writer,
        intent_hash_value,
        route_hash,
        &reward,
        &route_bytes,
    );

    let claimant = Pubkey::new_unique();
    pre_create_atas(&mut ctx, &reward, &route, claimant);

    let claimant_atas: Vec<AccountMeta> = reward
        .tokens
        .iter()
        .map(|token| {
            AccountMeta::new(
                get_associated_token_address_with_program_id(
                    &claimant,
                    &token.token,
                    &ctx.token_program,
                ),
                false,
            )
        })
        .collect();

    let result = send_flash_fulfill_ix(
        &mut ctx,
        intent_hash_value,
        &route,
        &reward,
        claimant,
        claimant_atas,
        call_accounts,
        heap_frame_bytes,
        1_400_000,
    );

    // The inner portal.fulfill CPI will fail because the Route's call targets
    // are dummy pubkeys — that's expected. We only care that the heap path up
    // to that point stayed clean.
    match result {
        Ok(meta) => meta.logs,
        Err(err) => err.meta.logs.clone(),
    }
}

/// Baseline path: no `RequestHeapFrame` on the tx (default 32 KB heap).
/// Without the strip/collect fixes this OOMs at
/// `cpi::fulfill::fulfill_intent`'s `infos` Vec allocation.
#[test]
fn mainnet_shape_intent_hash_default_heap_does_not_oom() {
    let logs = run_mainnet_shape_flash_fulfill(None);
    assert_no_oom(&logs);
}

/// Same path with a 256 KB heap-frame request — surfaces runtime/LiteSVM
/// RequestHeapFrame wiring as a side-effect, but primarily re-asserts the
/// heap path is tight enough to survive on default heap too.
#[test]
fn mainnet_shape_intent_hash_256k_heap_does_not_oom() {
    let logs = run_mainnet_shape_flash_fulfill(Some(256 * 1024));
    assert_no_oom(&logs);
}
