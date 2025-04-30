// --- intent_hash.rs -------------------------------------------------
use ethers_core::abi::{decode, encode, ParamType, Token};
use ethers_core::types::U256;
use tiny_keccak::{Hasher, Keccak};

use crate::error::EcoRoutesError;
use crate::state::{Call, Reward, Route, TokenAmount};

/// Solidity's keccak256 helper
#[inline(always)]
fn keccak(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut hasher = Keccak::v256();
    hasher.update(data);
    hasher.finalize(&mut out);
    out
}

fn u256(v: u64) -> Token {
    // everything that is an int / uint in Solidity occupies 32 bytes
    Token::Uint(U256::from(v))
}

fn fixed_bytes32(b: &[u8; 32]) -> Token {
    Token::FixedBytes(b.to_vec()) // equivalent to Solidity bytes32
}

fn abi_token_amount(t: &TokenAmount) -> Token {
    Token::Tuple(vec![
        fixed_bytes32(&t.token), // address -> bytes32 on the Solana side
        u256(t.amount as u64),   // amount (uint256)
    ])
}

fn abi_call(c: &Call) -> Token {
    Token::Tuple(vec![
        fixed_bytes32(&c.destination),    // target
        Token::Bytes(c.calldata.clone()), // data (bytes - dynamic)
        u256(0),                          // value – EVM has it, Solana side is always 0
    ])
}

fn abi_route(r: &Route) -> Vec<u8> {
    let tokens: Vec<Token> = r.tokens.iter().map(abi_token_amount).collect();
    let calls: Vec<Token> = r.calls.iter().map(abi_call).collect();

    encode(&[Token::Tuple(vec![
        fixed_bytes32(&r.salt), // salt (32 bytes) – we don't keep salt; put 0s
        u256(r.source_domain_id as u64), // source  (uint256)
        u256(r.destination_domain_id as u64), // destination (uint256)
        fixed_bytes32(&r.inbox), // inbox  (bytes32)
        Token::Array(tokens),   // TokenAmount[]
        Token::Array(calls),    // Call[]
    ])])
}

fn abi_reward(r: &Reward) -> Vec<u8> {
    let tokens: Vec<Token> = r.tokens.iter().map(abi_token_amount).collect();

    encode(&[Token::Tuple(vec![
        fixed_bytes32(&r.creator.to_bytes()), // creator  (bytes32)
        fixed_bytes32(&r.prover),             // prover   (bytes32)
        u256(r.deadline as u64),              // deadline (uint256)
        u256(r.native_amount as u64),         // nativeValue (uint256)
        Token::Array(tokens),                 // TokenAmount[]
    ])])
}

pub fn get_intent_hash(route: &Route, reward: &Reward) -> [u8; 32] {
    let route_hash = keccak(&abi_route(route)); // keccak256(abi.encode(route))
    let reward_hash = keccak(&abi_reward(reward)); // keccak256(abi.encode(reward))

    let mut packed = [0u8; 64];
    packed[..32].copy_from_slice(&route_hash);
    packed[32..].copy_from_slice(&reward_hash);

    keccak(&packed) // `intentHash  = keccak256(abi.encodePacked(routeHash, rewardHash));`
}

pub fn encode_fulfillment_message(
    intent_hashes: &[[u8; 32]],
    solvers: &[[u8; 32]], // 32-byte solvers
) -> Vec<u8> {
    assert_eq!(intent_hashes.len(), solvers.len(), "length mismatch");

    let hash_tokens = intent_hashes
        .iter()
        .map(|h| Token::FixedBytes(h.to_vec()))
        .collect::<Vec<_>>();

    let solver_tokens = solvers
        .iter()
        .map(|c| Token::FixedBytes(c.to_vec()))
        .collect::<Vec<_>>();

    encode(&[Token::Array(hash_tokens), Token::Array(solver_tokens)])
}

pub fn decode_fulfillment_message(
    data: &[u8],
) -> anchor_lang::Result<(Vec<[u8; 32]>, Vec<[u8; 32]>)> {
    let schema_fixed = vec![
        ParamType::Array(Box::new(ParamType::FixedBytes(32))),
        ParamType::Array(Box::new(ParamType::FixedBytes(32))),
    ];

    let tokens = decode(&schema_fixed, data).map_err(|_| EcoRoutesError::InvalidHandlePayload)?;

    if let (Some(Token::Array(h)), Some(Token::Array(c))) = (tokens.get(0), tokens.get(1)) {
        let hashes = h.iter().filter_map(as_bytes32).collect::<Vec<_>>();
        let claims = c.iter().filter_map(as_bytes32).collect::<Vec<_>>();
        if hashes.len() == h.len() && claims.len() == c.len() {
            return Ok((hashes, claims));
        }
    }

    Err(EcoRoutesError::InvalidHandlePayload.into())
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
