//! Integration tests for the chunked-buffer flash-fulfill flow:
//! `init_flash_fulfill_intent` + `append_flash_fulfill_route_chunk`, plus
//! the `cancel`/`close_abandoned` escape hatches. The single-tx convenience
//! `set_flash_fulfill_intent` is covered in `set_flash_fulfill_intent.rs`.

use anchor_lang::error::ErrorCode;
use anchor_lang::AnchorSerialize;
use eco_svm_std::{Bytes32, CHAIN_ID};
use flash_fulfiller::instructions::{
    AppendFlashFulfillRouteChunkArgs, FlashFulfillerError, InitFlashFulfillIntentArgs,
};
use flash_fulfiller::state::{FlashFulfillIntentAccount, ABANDON_TTL_SECS};
use portal::types::intent_hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

pub mod common;

// ---------- init_flash_fulfill_intent ----------

#[test]
fn init_preimage_mismatch_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value, &writer.pubkey()).0;

    // Supply bogus intent_hash unrelated to the real (route_hash, reward) preimage.
    let bogus_intent_hash = Bytes32::from([0xFFu8; 32]);

    let result = ctx.flash_fulfiller().init_flash_fulfill_intent(
        &writer,
        buffer,
        InitFlashFulfillIntentArgs {
            intent_hash: bogus_intent_hash,
            route_hash: route.hash(),
            reward: reward.clone(),
            route_total_size: route.try_to_vec().unwrap().len() as u32,
        },
    );

    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidIntentHash)));
}

#[test]
fn init_route_total_size_zero_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value, &writer.pubkey()).0;

    let result = ctx.flash_fulfiller().init_flash_fulfill_intent(
        &writer,
        buffer,
        InitFlashFulfillIntentArgs {
            intent_hash: intent_hash_value,
            route_hash: route.hash(),
            reward,
            route_total_size: 0,
        },
    );

    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidRouteTotalSize)));
}

#[test]
fn init_route_total_size_too_large_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value, &writer.pubkey()).0;

    let result = ctx.flash_fulfiller().init_flash_fulfill_intent(
        &writer,
        buffer,
        InitFlashFulfillIntentArgs {
            intent_hash: intent_hash_value,
            route_hash: route.hash(),
            reward,
            route_total_size: u32::MAX,
        },
    );

    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidRouteTotalSize)));
}

#[test]
fn init_tolerates_pre_funded_pda() {
    // An attacker who knows (intent_hash, writer_key) can pre-fund the
    // buffer PDA with a lamport transfer to block `create_account`. `do_init`
    // falls back to allocate+assign in that case so the legitimate writer
    // can still initialize.
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let writer = ctx.payer.insecure_clone();
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value, &writer.pubkey()).0;

    // Pre-fund the PDA with a tiny amount (simulates grief).
    ctx.airdrop(&buffer, 1).unwrap();

    let result = ctx.flash_fulfiller().init_flash_fulfill_intent(
        &writer,
        buffer,
        InitFlashFulfillIntentArgs {
            intent_hash: intent_hash_value,
            route_hash: route.hash(),
            reward,
            route_total_size: route.try_to_vec().unwrap().len() as u32,
        },
    );
    assert!(result.is_ok());

    let stored = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert_eq!(stored.writer, writer.pubkey());
}

#[test]
fn init_pda_binds_to_writer() {
    // An attacker holding the public (route_hash, reward) preimage cannot
    // squat on a legitimate writer's PDA — the writer is part of the seed.
    let mut ctx = common::Context::default();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let legitimate_writer = ctx.payer.insecure_clone();
    let legitimate_buffer =
        FlashFulfillIntentAccount::pda(&intent_hash_value, &legitimate_writer.pubkey()).0;

    let attacker = Keypair::new();
    ctx.airdrop(&attacker.pubkey(), common::sol_amount(1.0))
        .unwrap();

    // Attacker tries to init at legitimate writer's PDA — fails PDA check.
    let result = ctx.flash_fulfiller().init_flash_fulfill_intent(
        &attacker,
        legitimate_buffer,
        InitFlashFulfillIntentArgs {
            intent_hash: intent_hash_value,
            route_hash: route.hash(),
            reward: reward.clone(),
            route_total_size: route.try_to_vec().unwrap().len() as u32,
        },
    );
    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::InvalidFlashFulfillIntentAccount,
    )));

    // Legitimate writer can still init their own PDA unimpeded.
    let legitimate_result = ctx.flash_fulfiller().init_flash_fulfill_intent(
        &legitimate_writer,
        legitimate_buffer,
        InitFlashFulfillIntentArgs {
            intent_hash: intent_hash_value,
            route_hash: route.hash(),
            reward,
            route_total_size: route.try_to_vec().unwrap().len() as u32,
        },
    );
    assert!(legitimate_result.is_ok());
}

// ---------- append_flash_fulfill_route_chunk ----------

fn init_buffer_for_appends(
    ctx: &mut common::Context,
    writer: &Keypair,
) -> (
    Bytes32,
    Pubkey,
    Vec<u8>,
    portal::types::Route,
    portal::types::Reward,
) {
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    route.calls.clear();

    let intent_hash_value = intent_hash(CHAIN_ID, &route.hash(), &reward.hash());
    let buffer = FlashFulfillIntentAccount::pda(&intent_hash_value, &writer.pubkey()).0;
    let route_bytes = route.try_to_vec().unwrap();
    let route_total_size = route_bytes.len() as u32;

    ctx.flash_fulfiller()
        .init_flash_fulfill_intent(
            writer,
            buffer,
            InitFlashFulfillIntentArgs {
                intent_hash: intent_hash_value,
                route_hash: route.hash(),
                reward: reward.clone(),
                route_total_size,
            },
        )
        .unwrap();

    (intent_hash_value, buffer, route_bytes, route, reward)
}

#[test]
fn append_multi_chunk_happy_path_finalizes() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, route_bytes, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    // Split into two chunks.
    let split = route_bytes.len() / 2;
    let (first, second) = route_bytes.split_at(split);

    ctx.flash_fulfiller()
        .append_flash_fulfill_route_chunk(
            &writer,
            buffer,
            AppendFlashFulfillRouteChunkArgs {
                intent_hash: intent_hash_value,
                offset: 0,
                chunk: first.to_vec(),
            },
        )
        .unwrap();

    let after_first = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert!(!after_first.finalized);
    assert_eq!(after_first.route_bytes_written as usize, split);

    ctx.flash_fulfiller()
        .append_flash_fulfill_route_chunk(
            &writer,
            buffer,
            AppendFlashFulfillRouteChunkArgs {
                intent_hash: intent_hash_value,
                offset: split as u32,
                chunk: second.to_vec(),
            },
        )
        .unwrap();

    let after_final = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert!(after_final.finalized);
    assert_eq!(
        after_final.route_bytes_written,
        after_final.route_total_size
    );
}

#[test]
fn append_non_writer_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, route_bytes, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    let attacker = Keypair::new();
    ctx.airdrop(&attacker.pubkey(), common::sol_amount(1.0))
        .unwrap();

    // Wrong signer — seed derivation uses writer.key() so the PDA for the
    // attacker does not match the legitimate buffer's address.
    let result = ctx.flash_fulfiller().append_flash_fulfill_route_chunk(
        &attacker,
        buffer,
        AppendFlashFulfillRouteChunkArgs {
            intent_hash: intent_hash_value,
            offset: 0,
            chunk: route_bytes,
        },
    );
    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintSeeds)));
}

#[test]
fn append_wrong_offset_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, _, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    let result = ctx.flash_fulfiller().append_flash_fulfill_route_chunk(
        &writer,
        buffer,
        AppendFlashFulfillRouteChunkArgs {
            intent_hash: intent_hash_value,
            offset: 7, // not zero
            chunk: vec![0u8; 8],
        },
    );
    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidAppendOffset)));
}

#[test]
fn append_overflow_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, route_bytes, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    // Try to append a chunk that overflows the declared total size.
    let oversized = vec![0u8; route_bytes.len() + 1];
    let result = ctx.flash_fulfiller().append_flash_fulfill_route_chunk(
        &writer,
        buffer,
        AppendFlashFulfillRouteChunkArgs {
            intent_hash: intent_hash_value,
            offset: 0,
            chunk: oversized,
        },
    );
    assert!(result.is_err_and(common::is_error(FlashFulfillerError::AppendOverflow)));
}

#[test]
fn append_hash_mismatch_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, route_bytes, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    // Fill with zeros of the correct length — keccak won't match route_hash.
    let garbage = vec![0u8; route_bytes.len()];
    let result = ctx.flash_fulfiller().append_flash_fulfill_route_chunk(
        &writer,
        buffer,
        AppendFlashFulfillRouteChunkArgs {
            intent_hash: intent_hash_value,
            offset: 0,
            chunk: garbage,
        },
    );
    assert!(result.is_err_and(common::is_error(FlashFulfillerError::RouteHashMismatch)));

    // Buffer remains un-finalized; writer can still retry with correct bytes.
    let after = ctx.account::<FlashFulfillIntentAccount>(&buffer).unwrap();
    assert!(!after.finalized);
    assert_eq!(after.route_bytes_written, 0);
}

#[test]
fn append_after_finalize_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, route_bytes, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    ctx.flash_fulfiller()
        .append_flash_fulfill_route_chunk(
            &writer,
            buffer,
            AppendFlashFulfillRouteChunkArgs {
                intent_hash: intent_hash_value,
                offset: 0,
                chunk: route_bytes,
            },
        )
        .unwrap();

    let result = ctx.flash_fulfiller().append_flash_fulfill_route_chunk(
        &writer,
        buffer,
        AppendFlashFulfillRouteChunkArgs {
            intent_hash: intent_hash_value,
            offset: 0,
            chunk: vec![0u8; 4],
        },
    );
    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::BufferAlreadyFinalized
    )));
}

// ---------- cancel_flash_fulfill_intent ----------

#[test]
fn cancel_by_writer_succeeds_and_refunds_rent() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, _, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    let balance_before = ctx.balance(&writer.pubkey());
    let rent_in_buffer = ctx.balance(&buffer);

    ctx.flash_fulfiller()
        .cancel_flash_fulfill_intent(&writer, buffer, intent_hash_value)
        .unwrap();

    assert!(ctx.account::<FlashFulfillIntentAccount>(&buffer).is_none());
    let balance_after = ctx.balance(&writer.pubkey());
    // Writer recovers the rent (net of tx fee; so it's >= balance_before + rent - fee).
    assert!(balance_after >= balance_before + rent_in_buffer - common::sol_amount(0.001));
}

#[test]
fn cancel_by_non_writer_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, _, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    let attacker = Keypair::new();
    ctx.airdrop(&attacker.pubkey(), common::sol_amount(1.0))
        .unwrap();

    let result =
        ctx.flash_fulfiller()
            .cancel_flash_fulfill_intent(&attacker, buffer, intent_hash_value);
    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintSeeds)));
}

#[test]
fn cancel_after_finalize_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, route_bytes, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    ctx.flash_fulfiller()
        .append_flash_fulfill_route_chunk(
            &writer,
            buffer,
            AppendFlashFulfillRouteChunkArgs {
                intent_hash: intent_hash_value,
                offset: 0,
                chunk: route_bytes,
            },
        )
        .unwrap();

    let result =
        ctx.flash_fulfiller()
            .cancel_flash_fulfill_intent(&writer, buffer, intent_hash_value);
    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::BufferAlreadyFinalized
    )));
}

// ---------- close_abandoned_flash_fulfill_intent ----------

#[test]
fn close_abandoned_before_ttl_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, _, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    let caller = Keypair::new();
    ctx.airdrop(&caller.pubkey(), common::sol_amount(1.0))
        .unwrap();

    let result = ctx.flash_fulfiller().close_abandoned_flash_fulfill_intent(
        &caller,
        writer.pubkey(),
        buffer,
        intent_hash_value,
    );
    assert!(result.is_err_and(common::is_error(FlashFulfillerError::NotAbandonedYet)));
}

#[test]
fn close_abandoned_after_ttl_refunds_writer() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, _, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    let caller = Keypair::new();
    ctx.airdrop(&caller.pubkey(), common::sol_amount(1.0))
        .unwrap();

    // Warp past the abandonment TTL.
    let now_ts = ctx.now() as i64;
    ctx.warp_to_timestamp(now_ts + ABANDON_TTL_SECS + 1);

    let writer_before = ctx.balance(&writer.pubkey());
    let rent = ctx.balance(&buffer);

    ctx.flash_fulfiller()
        .close_abandoned_flash_fulfill_intent(&caller, writer.pubkey(), buffer, intent_hash_value)
        .unwrap();

    assert!(ctx.account::<FlashFulfillIntentAccount>(&buffer).is_none());
    let writer_after = ctx.balance(&writer.pubkey());
    assert_eq!(writer_after, writer_before + rent);
}

#[test]
fn close_abandoned_wrong_writer_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, _, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    let caller = Keypair::new();
    ctx.airdrop(&caller.pubkey(), common::sol_amount(1.0))
        .unwrap();

    let now_ts = ctx.now() as i64;
    ctx.warp_to_timestamp(now_ts + ABANDON_TTL_SECS + 1);

    // Wrong writer pubkey: seed derivation uses the supplied writer, so the
    // resulting PDA does not match the legitimate buffer's address.
    let impostor = Pubkey::new_unique();
    let result = ctx.flash_fulfiller().close_abandoned_flash_fulfill_intent(
        &caller,
        impostor,
        buffer,
        intent_hash_value,
    );
    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintSeeds)));
}

#[test]
fn close_abandoned_on_finalized_fail() {
    let mut ctx = common::Context::default();
    let writer = ctx.payer.insecure_clone();
    let (intent_hash_value, buffer, route_bytes, _, _) = init_buffer_for_appends(&mut ctx, &writer);

    ctx.flash_fulfiller()
        .append_flash_fulfill_route_chunk(
            &writer,
            buffer,
            AppendFlashFulfillRouteChunkArgs {
                intent_hash: intent_hash_value,
                offset: 0,
                chunk: route_bytes,
            },
        )
        .unwrap();

    let caller = Keypair::new();
    ctx.airdrop(&caller.pubkey(), common::sol_amount(1.0))
        .unwrap();

    let now_ts = ctx.now() as i64;
    ctx.warp_to_timestamp(now_ts + ABANDON_TTL_SECS + 1);

    let result = ctx.flash_fulfiller().close_abandoned_flash_fulfill_intent(
        &caller,
        writer.pubkey(),
        buffer,
        intent_hash_value,
    );
    assert!(result.is_err_and(common::is_error(
        FlashFulfillerError::BufferAlreadyFinalized
    )));
}
