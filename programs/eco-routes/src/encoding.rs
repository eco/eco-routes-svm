use crate::{
    error::EcoRoutesError,
    state::{Reward, Route, TokenAmount},
};
use ethabi::{decode, encode, ParamType, Token};
use tiny_keccak::{Hasher, Keccak};

#[inline(always)]
fn u256_be(v: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

/// Mirrors: keccak256( abi.encode(
///    bytes32 salt,
///    uint256 source,
///    uint256 destination,
///    bytes32 inbox,
///    (bytes32,uint256)[] tokens,
///    (bytes32,bytes,uint256)[] calls
/// ) )
pub fn hash_route(route: &Route) -> [u8; 32] {
    // 1) Build the "head" (6 * 32 bytes):
    //      [ salt               ] (bytes32)
    //      [ source_domain_id   ] (uint256)
    //      [ destination_domain_id ] (uint256)
    //      [ inbox              ] (bytes32)
    //      [ offset_to_tokens   ] (uint256)
    //      [ offset_to_calls    ] (uint256)
    //
    // 2) Build the “tail”:
    //      – First: tokens array (length + each (bytes32,uint256))
    //      – Then: calls array (length + each (bytes32,bytes,uint256))
    //
    // 3) Prepend a 32-byte "0x20" (offset to tuple body) to emulate `abi.encode(&[route_token])`.

    // head
    let mut head = Vec::with_capacity(6 * 32);
    head.extend_from_slice(&route.salt); // bytes32
    head.extend_from_slice(&u256_be(route.source_domain_id as u64)); // uint256
    head.extend_from_slice(&u256_be(route.destination_domain_id as u64)); // uint256
    head.extend_from_slice(&route.inbox); // bytes32

    // tokens_offset = 6 * 32 = where the “tokens” array (length + content) begins
    let tokens_offset = 6 * 32;
    head.extend_from_slice(&u256_be(tokens_offset as u64)); // offset to tokens

    // Build tokens_tail (dynamic)
    //    (a) 32-byte length word
    //    (b) for each TokenAmount: [token (bytes32)] [ amount (uint256) ]
    let mut tokens_tail = Vec::with_capacity(32 + route.tokens.len() * 64);
    tokens_tail.extend_from_slice(&u256_be(route.tokens.len() as u64));
    for TokenAmount { token, amount } in &route.tokens {
        tokens_tail.extend_from_slice(token); // bytes32
        tokens_tail.extend_from_slice(&u256_be(*amount)); // uint256
    }

    // calls_offset = tokens_offset + size_of(tokens_tail)
    let calls_offset = tokens_offset + tokens_tail.len();
    head.extend_from_slice(&u256_be(calls_offset as u64)); // offset to calls

    // Tail: start with tokens_tail
    let mut tail = tokens_tail;

    // ---------------------------------------------------------------
    // Build “calls_tail”.
    //
    // ABI layout for (bytes32,bytes,uint256)[]:
    //   calls_tail :=
    //     [ length ]                         -- 32 bytes
    //     [ offset_0 ] … [ offset_{n-1} ]    -- n * 32
    //     tuple_0 | tuple_1 | … | tuple_{n-1}
    //
    //   tuple_i :=
    //     [ destination ]                    -- bytes32
    //     [ 0x60 ]                           -- offset to calldata inside tuple (= 96)
    //     [ value ]                          -- uint256 (always 0)
    //     [ calldata_len ]                   -- uint256
    //     [ calldata bytes ] + padding
    // ---------------------------------------------------------------
    let mut calls_tail = Vec::new();
    let n_calls = route.calls.len();
    calls_tail.extend_from_slice(&u256_be(n_calls as u64)); // array length

    if n_calls > 0 {
        // first collect each tuple’s encoded bytes so we know their sizes
        let mut tuples = Vec::new();
        for call in &route.calls {
            // head (destination, offset-to-bytes, value)
            let mut tup = Vec::with_capacity(96);
            tup.extend_from_slice(&call.destination); // bytes32
            tup.extend_from_slice(&u256_be(96)); // offset to bytes
            tup.extend_from_slice(&u256_be(0)); // value == 0

            // dynamic bytes
            let len = call.calldata.len();
            tup.extend_from_slice(&u256_be(len as u64)); // bytes length
            tup.extend_from_slice(&call.calldata); // bytes payload
            let pad = (32 - (len % 32)) % 32; // right-pad to 32-byte boundary
            tup.extend(std::iter::repeat(0u8).take(pad));

            tuples.push(tup);
        }

        // compute offsets
        let head_size = n_calls * 32;
        let mut running_size = 0usize;
        for tup in &tuples {
            let offset = head_size + running_size;
            calls_tail.extend_from_slice(&u256_be(offset as u64)); // offset_i
            running_size += tup.len();
        }

        // append the tuples themselves
        for tup in tuples {
            calls_tail.extend_from_slice(&tup);
        }
    }

    // Append calls_tail (which is just the 32-byte zero length) to tail.
    tail.extend_from_slice(&calls_tail);

    // full "abi.encode(&[route_token])" layout
    //     [0..0 | 0x20]  // 32-byte "offset to the tuple body"  (0x20)
    //     | head
    //     | tail

    let mut full = Vec::with_capacity(32 + head.len() + tail.len());
    full.extend_from_slice(&u256_be(32)); // the "0x20" word
    full.extend_from_slice(&head);
    full.extend_from_slice(&tail);

    // Keccak256 of that:
    let mut hasher = Keccak::v256();
    hasher.update(&full);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

/// Mirrors: keccak256( abi.encode(
///    bytes32 creator,
///    bytes32 prover,
///    uint256 deadline,
///    uint256 nativeValue,
///    (bytes32,uint256)[] tokens
/// ) )
pub fn hash_reward(r: &Reward) -> [u8; 32] {
    // HEAD (5 * 32):
    //    [creator (bytes32)]
    //    [prover   (bytes32)]
    //    [deadline (uint256)]
    //    [nativeValue (uint256)]
    //    [offset_to_tokens (uint256)]
    //
    // TAIL (tokens array):
    //    [ tokens.length (uint256) ]
    //    [ for each token: (bytes32, uint256) ]
    //
    // Then wrap it in the “0x20” offset word to simulate `encode(&[reward_token])`.

    let mut head = Vec::with_capacity(5 * 32);
    head.extend_from_slice(&r.creator.to_bytes());
    head.extend_from_slice(&r.prover);
    head.extend_from_slice(&u256_be(r.deadline as u64));
    head.extend_from_slice(&u256_be(r.native_amount));

    // tokens_offset = 5 * 32
    let tokens_offset = 5 * 32;
    head.extend_from_slice(&u256_be(tokens_offset as u64));

    // TAIL: tokens array
    let mut tail = Vec::new();
    tail.extend_from_slice(&u256_be(r.tokens.len() as u64));
    for TokenAmount { token, amount } in &r.tokens {
        tail.extend_from_slice(token); // bytes32
        tail.extend_from_slice(&u256_be(*amount)); // uint256
    }

    // Build final: [0x20 offset] | head | tail
    let mut full = Vec::with_capacity(32 + head.len() + tail.len());
    full.extend_from_slice(&u256_be(32)); // top-level offset
    full.extend_from_slice(&head);
    full.extend_from_slice(&tail);

    let mut hasher = Keccak::v256();
    hasher.update(&full);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

// intent = keccak256( routeHash | rewardHash )
pub fn get_intent_hash(route: &Route, reward: &Reward) -> [u8; 32] {
    let rh = hash_route(route);
    let rwd = hash_reward(reward);
    let mut hasher = Keccak::v256();
    hasher.update(&rh);
    hasher.update(&rwd);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

pub fn encode_fulfillment_message(intent_hashes: &[[u8; 32]], solvers: &[[u8; 32]]) -> Vec<u8> {
    assert_eq!(intent_hashes.len(), solvers.len(), "length mismatch");

    let mut hash_tokens = Vec::with_capacity(intent_hashes.len());
    for hash in intent_hashes {
        hash_tokens.push(Token::FixedBytes(hash[..].into()));
    }

    let mut solver_tokens = Vec::with_capacity(solvers.len());
    for solver in solvers {
        solver_tokens.push(Token::FixedBytes(solver[..].into()));
    }

    encode(&[Token::Array(hash_tokens), Token::Array(solver_tokens)])
}

pub fn decode_fulfillment_message(
    data: &[u8],
) -> anchor_lang::Result<(Vec<[u8; 32]>, Vec<[u8; 32]>)> {
    let schema: &[ParamType] = &[
        ParamType::Array(Box::new(ParamType::FixedBytes(32))),
        ParamType::Array(Box::new(ParamType::FixedBytes(32))),
    ];

    let tokens = decode(schema, data).map_err(|_| EcoRoutesError::InvalidHandlePayload)?;

    match tokens.as_slice() {
        [Token::Array(hashes), Token::Array(claims)] => {
            let mut h_out = Vec::with_capacity(hashes.len());
            let mut c_out = Vec::with_capacity(claims.len());

            for token in hashes {
                if let Some(val) = as_bytes32(&token) {
                    h_out.push(val);
                } else {
                    return Err(EcoRoutesError::InvalidHandlePayload.into());
                }
            }

            for token in claims {
                if let Some(val) = as_bytes32(&token) {
                    c_out.push(val);
                } else {
                    return Err(EcoRoutesError::InvalidHandlePayload.into());
                }
            }

            Ok((h_out, c_out))
        }
        _ => Err(EcoRoutesError::InvalidHandlePayload.into()),
    }
}

#[inline(always)]
fn as_bytes32(token: &Token) -> Option<[u8; 32]> {
    if let Token::FixedBytes(bytes) = token {
        if bytes.len() == 32 {
            let mut out = [0u8; 32];
            out.copy_from_slice(bytes);
            return Some(out);
        }
    }
    None
}
