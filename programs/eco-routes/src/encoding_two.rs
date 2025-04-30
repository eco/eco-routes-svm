use anchor_lang::prelude::*;
use tiny_keccak::{Hasher, Keccak};

use crate::state::{Call, Reward, Route, TokenAmount};

/// --- helpers -------------------------------------------------------------

#[inline(always)]
fn u256_be(v: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

#[inline(always)]
fn feed(hasher: &mut Keccak, bytes: &[u8]) {
    hasher.update(bytes);
}

#[inline(always)]
fn feed_word(hasher: &mut Keccak, word: &[u8; 32]) {
    feed(hasher, word);
}

#[inline(always)]
fn feed_pad32(hasher: &mut Keccak, bytes: &[u8]) {
    let mut tmp = [0u8; 32];
    tmp[..bytes.len()].copy_from_slice(bytes);
    feed(hasher, &tmp);
}

/// Round `len` up to the next multiple of 32.
#[inline(always)]
fn padded(len: usize) -> usize {
    (len + 31) & !31
}

/// --- Route ---------------------------------------------------------------

fn hash_route(route: &Route) -> [u8; 32] {
    // pre-compute array sizes to calculate offsets -------------------------
    let token_array_size = 32 + route.tokens.len() * 64; // len + n·TokenAmount(2×32)

    let mut call_sizes = Vec::with_capacity(route.calls.len());
    let mut call_tail_total = 0;

    for c in &route.calls {
        let data_padded = padded(c.calldata.len());
        let size = 96                // head of the tuple (dest, offset, value)
            + 32 + data_padded; // bytes data (len + padded bytes)
        call_sizes.push(size);
        call_tail_total += size;
    }

    let calls_head_size = 32 + 32 * route.calls.len(); // len + offsets
    let calls_array_size = calls_head_size + call_tail_total;

    // offsets (relative to start of Route encoding)
    let tokens_offset = 32 * 6; // after 6 static words
    let calls_offset = tokens_offset + token_array_size;

    // ----------------------------------------------------------------------
    let mut k = Keccak::v256();

    // 1) Route head (6 words) ---------------------------------------------
    feed_word(&mut k, &route.salt);
    feed_word(&mut k, &u256_be(route.source_domain_id as u64));
    feed_word(&mut k, &u256_be(route.destination_domain_id as u64));
    feed_word(&mut k, &route.inbox);
    feed_word(&mut k, &u256_be(tokens_offset as u64));
    feed_word(&mut k, &u256_be(calls_offset as u64));

    // 2) TokenAmount[] -----------------------------------------------------
    feed_word(&mut k, &u256_be(route.tokens.len() as u64));
    for t in &route.tokens {
        feed_word(&mut k, &t.token);
        feed_word(&mut k, &u256_be(t.amount));
    }

    // 3) Call[]  – head ----------------------------------------------------
    feed_word(&mut k, &u256_be(route.calls.len() as u64));

    let mut running = calls_head_size; // first tail offset inside Call[]
    for sz in &call_sizes {
        feed_word(&mut k, &u256_be(running as u64));
        running += *sz;
    }

    // 4) Call[] – tail (each Call tuple) ----------------------------------
    for (call, sz) in route.calls.iter().zip(call_sizes.iter()) {
        // tuple head
        feed_word(&mut k, &call.destination);
        feed_word(&mut k, &u256_be(96)); // offset to bytes = 0x60
        feed_word(&mut k, &u256_be(0)); // value == 0

        // bytes calldata
        feed_word(&mut k, &u256_be(call.calldata.len() as u64));
        feed_word(&mut k, &u256_be(call.calldata.len() as u64));
        for chunk in call.calldata.chunks(32) {
            if chunk.len() == 32 {
                feed(&mut k, chunk);
            } else {
                feed_pad32(&mut k, chunk); // last chunk padded once – done.
            }
        }
        // padding already handled in loop
    }

    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

/// --- Reward --------------------------------------------------------------

fn hash_reward(r: &Reward) -> [u8; 32] {
    let token_array_size = 32 + r.tokens.len() * 64; // len + tokens
    let tokens_offset = 32 * 5; // head words = 5

    let mut k = Keccak::v256();

    feed_word(&mut k, &r.creator.to_bytes());
    feed_word(&mut k, &r.prover);
    feed_word(&mut k, &u256_be(r.deadline as u64));
    feed_word(&mut k, &u256_be(r.native_amount));
    feed_word(&mut k, &u256_be(tokens_offset as u64));

    feed_word(&mut k, &u256_be(r.tokens.len() as u64));
    for t in &r.tokens {
        feed_word(&mut k, &t.token);
        feed_word(&mut k, &u256_be(t.amount));
    }

    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

/// Public API – unchanged --------------------------------------------------
pub fn get_intent_hash(route: &Route, reward: &Reward) -> [u8; 32] {
    use anchor_lang::solana_program::keccak;
    keccak::hashv(&[&hash_route(route), &hash_reward(reward)]).0
}
