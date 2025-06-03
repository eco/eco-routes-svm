use anchor_lang::{require, Result};
use derive_more::Deref;
use ethers_core::abi::{decode, encode, ParamType, Token};
use tiny_keccak::{Hasher, Keccak};

use crate::{
    error::EcoRoutesError,
    state::{Reward, Route},
};

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

#[inline(always)]
fn padded(len: usize) -> usize {
    (len + 31) & !31
}

fn hash_route(route: &Route) -> [u8; 32] {
    let token_array_size = 32 + route.tokens.len() * 64;

    let mut call_sizes = Vec::with_capacity(route.calls.len());

    for call in &route.calls {
        let data_padded = padded(call.calldata.len());
        let size = 96 + 32 + data_padded;
        call_sizes.push(size);
    }

    let calls_head_size = 32 + 32 * route.calls.len();

    let tokens_offset = 32 * 6;
    let calls_offset = tokens_offset + token_array_size;

    let mut k = Keccak::v256();

    feed_word(&mut k, &route.salt);
    feed_word(&mut k, &u256_be(route.source_domain_id as u64));
    feed_word(&mut k, &u256_be(route.destination_domain_id as u64));
    feed_word(&mut k, &route.inbox);
    feed_word(&mut k, &u256_be(tokens_offset as u64));
    feed_word(&mut k, &u256_be(calls_offset as u64));

    feed_word(&mut k, &u256_be(route.tokens.len() as u64));
    for t in &route.tokens {
        feed_word(&mut k, &t.token);
        feed_word(&mut k, &u256_be(t.amount));
    }

    feed_word(&mut k, &u256_be(route.calls.len() as u64));

    let mut running = calls_head_size;
    for sz in &call_sizes {
        feed_word(&mut k, &u256_be(running as u64));
        running += *sz;
    }

    for (call, _sz) in route.calls.iter().zip(call_sizes.iter()) {
        feed_word(&mut k, &call.destination);
        feed_word(&mut k, &u256_be(96));
        feed_word(&mut k, &u256_be(0));

        feed_word(&mut k, &u256_be(call.calldata.len() as u64));
        feed_word(&mut k, &u256_be(call.calldata.len() as u64));
        for chunk in call.calldata.chunks(32) {
            if chunk.len() == 32 {
                feed(&mut k, chunk);
            } else {
                feed_pad32(&mut k, chunk);
            }
        }
    }

    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

fn hash_reward(r: &Reward) -> [u8; 32] {
    let tokens_offset = 32 * 5;

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

pub fn get_intent_hash(route: &Route, reward: &Reward) -> [u8; 32] {
    use anchor_lang::solana_program::keccak;
    keccak::hashv(&[&hash_route(route), &hash_reward(reward)]).0
}
#[derive(Deref)]
pub struct FulfillMessages(Vec<([u8; 32], [u8; 32])>);

impl FulfillMessages {
    pub fn new(intent_hashes: Vec<[u8; 32]>, solvers: Vec<[u8; 32]>) -> Result<Self> {
        require!(
            intent_hashes.len() == solvers.len(),
            EcoRoutesError::InvalidFulfillMessage
        );

        Ok(Self(intent_hashes.into_iter().zip(solvers).collect()))
    }

    pub fn intent_hashes(&self) -> Vec<[u8; 32]> {
        self.iter().map(|(intent_hash, _)| *intent_hash).collect()
    }

    pub fn solvers(&self) -> Vec<[u8; 32]> {
        self.iter().map(|(_, solver)| *solver).collect()
    }

    pub fn encode(&self) -> Vec<u8> {
        let (intent_hashes, solvers) = self
            .iter()
            .map(|(intent_hash, solver)| {
                (
                    Token::FixedBytes(intent_hash.to_vec()),
                    Token::FixedBytes(solver.to_vec()),
                )
            })
            .unzip();

        encode(&[Token::Array(intent_hashes), Token::Array(solvers)])
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let schema_fixed = vec![
            ParamType::Array(Box::new(ParamType::FixedBytes(32))),
            ParamType::Array(Box::new(ParamType::FixedBytes(32))),
        ];

        let tokens =
            decode(&schema_fixed, data).map_err(|_| EcoRoutesError::InvalidFulfillMessage)?;

        match &tokens[..] {
            [Token::Array(intent_hashes), Token::Array(solvers)] => {
                let intent_hashes = intent_hashes
                    .iter()
                    .filter_map(as_bytes32)
                    .collect::<Vec<_>>();
                let solvers = solvers.iter().filter_map(as_bytes32).collect::<Vec<_>>();

                Self::new(intent_hashes, solvers)
            }
            _ => Err(EcoRoutesError::InvalidFulfillMessage.into()),
        }
    }
}

fn as_bytes32(token: &Token) -> Option<[u8; 32]> {
    match token {
        Token::FixedBytes(v) if v.len() == 32 => {
            let mut out = [0u8; 32];
            out.copy_from_slice(v);
            Some(out)
        }
        _ => None,
    }
}
