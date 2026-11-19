//! Decoding of the EVM `IntentFulfilledFromSource(uint64 indexed source, bytes
//! encodedProofs)` event as Polymer returns it: flat 32-byte topics and the
//! ABI-encoded non-indexed data.
//!
//! The two halves are decoded by separate functions on purpose: `validate`
//! checks the topics (and the source chain they carry) before it touches the
//! payload, so a doubly-invalid event reports `InvalidSourceChain`, not
//! `InvalidEventData` (design spec, section 3.4, steps 3 and 4). This is an
//! SVM-local choice: PolymerProver.sol's `validate` shape-checks the payload
//! before comparing the selector, so the two legs diagnose the same malformed
//! event differently. The divergence is diagnostic only — both legs revert, and
//! the emitter whitelist gates both.

use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::instructions::PolymerProverError;

/// `keccak256("IntentFulfilledFromSource(uint64,bytes)")`
pub const INTENT_FULFILLED_FROM_SOURCE_SELECTOR: [u8; 32] = [
    0xd4, 0x93, 0xdd, 0xe4, 0xde, 0x24, 0x06, 0x6d, 0xb2, 0x9a, 0x75, 0x4b, 0x1e, 0x9d, 0xc5, 0xde,
    0xdf, 0x70, 0x84, 0x85, 0x0c, 0x4a, 0xbf, 0x76, 0xc2, 0x30, 0x46, 0xba, 0x9d, 0xad, 0x6b, 0x1d,
];
/// Two topics: the selector and the indexed `uint64 source`.
pub const TOPICS_LEN: usize = 64;
const WORD: usize = 32;

/// The three topic gates: two topics, our selector, and a `uint64` source
/// chain, returned for the caller to compare against `CHAIN_ID`. Never reads
/// `unindexed_data`.
pub fn parse_source(topics: &[u8]) -> Result<u64> {
    require!(
        topics.len() == TOPICS_LEN,
        PolymerProverError::InvalidTopicsLength
    );
    require!(
        topics[..WORD] == INTENT_FULFILLED_FROM_SOURCE_SELECTOR,
        PolymerProverError::InvalidEventSignature
    );
    uint64_word(&topics[WORD..TOPICS_LEN])
        .ok_or_else(|| PolymerProverError::InvalidSourceChain.into())
}

/// Unwraps `unindexed_data` as one ABI-encoded `bytes`: the `ProofData` bytes
/// (8-byte destination followed by (intent hash, claimant) pairs).
pub fn decode_encoded_proofs(unindexed_data: &[u8]) -> Result<Vec<u8>> {
    abi_decode_bytes(unindexed_data).ok_or_else(|| PolymerProverError::InvalidEventData.into())
}

/// Left-pads a 20-byte EVM address to the 32-byte form used by `Config`.
pub fn evm_address_to_bytes32(address: [u8; 20]) -> Bytes32 {
    let mut bytes = [0u8; 32];
    bytes[12..].copy_from_slice(&address);
    bytes.into()
}

/// ABI-encodes a single dynamic `bytes` value (offset word, length word, data
/// padded to a 32-byte boundary). Used by tests to build event data.
pub fn abi_encode_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 * WORD + data.len().div_ceil(WORD) * WORD);
    out.extend_from_slice(&[0u8; WORD - 8]);
    out.extend_from_slice(&(WORD as u64).to_be_bytes());
    out.extend_from_slice(&[0u8; WORD - 8]);
    out.extend_from_slice(&(data.len() as u64).to_be_bytes());
    out.extend_from_slice(data);
    out.resize(
        out.len() + (data.len().div_ceil(WORD) * WORD - data.len()),
        0,
    );
    out
}

/// Decodes a 32-byte ABI word whose value fits in a `u64` (top 24 bytes zero).
fn uint64_word(word: &[u8]) -> Option<u64> {
    if word.len() != WORD || word[..WORD - 8].iter().any(|byte| *byte != 0) {
        return None;
    }
    Some(u64::from_be_bytes(word[WORD - 8..].try_into().ok()?))
}

/// Inverse of [`abi_encode_bytes`]: returns the payload, or `None` if the
/// offset is not 32, the length word cannot be a valid padded length, or the
/// buffer is shorter than the padded payload.
fn abi_decode_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let offset = uint64_word(data.get(..WORD)?)?;
    if offset != WORD as u64 {
        return None;
    }
    let len = usize::try_from(uint64_word(data.get(WORD..2 * WORD)?)?).ok()?;
    // `len` is an untrusted 32-byte ABI word: keep every derived index checked
    // so an absurd length is `InvalidEventData`, not an overflow panic.
    let padded_len = len.div_ceil(WORD).checked_mul(WORD)?;
    let body = data.get(2 * WORD..)?;
    if body.len() < padded_len {
        return None;
    }
    Some(body.get(..len)?.to_vec())
}

#[cfg(test)]
mod tests {
    use solana_keccak_hasher::hash as keccak;

    use super::*;

    fn topics(selector: [u8; 32], source: u64) -> Vec<u8> {
        let mut topics = selector.to_vec();
        topics.extend_from_slice(&[0u8; 24]);
        topics.extend_from_slice(&source.to_be_bytes());
        topics
    }

    fn decode_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn selector_matches_keccak_of_signature() {
        let expected = keccak(b"IntentFulfilledFromSource(uint64,bytes)").to_bytes();
        assert_eq!(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, expected);
    }

    #[test]
    fn parse_success() {
        let payload = vec![0x11u8; 72];

        let source =
            parse_source(&topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1399811150)).unwrap();
        let encoded_proofs = decode_encoded_proofs(&abi_encode_bytes(&payload)).unwrap();

        assert_eq!(source, 1399811150);
        assert_eq!(encoded_proofs, payload);
    }

    /// The inbound half of the cross-repo wire format, pinned against bytes the
    /// Solidity ABI encoder produced rather than our own `abi_encode_bytes`:
    /// `cast keccak "IntentFulfilledFromSource(uint64,bytes)"`,
    /// `cast abi-encode "f(uint64)" 1399811150` for topic 1, and
    /// `cast abi-encode "f(bytes)" 0x<encodedProofs>` for the data — the same
    /// encoding `emit IntentFulfilledFromSource(source, encodedProofs)` writes.
    /// This fixture and the outbound golden
    /// (`prove/testdata/prove_log_line_format.golden`, mirrored in eco-routes'
    /// `SVM_GOLDEN_LOG`) are the two halves of the round trip, not the same
    /// intent: here the intent's source is Solana (`CHAIN_ID` = 0x536f6c4e, in
    /// topic 1) and its destination is 8453 (the leading word of
    /// `encodedProofs`, checked against `result.chain_id` in `validate`); the
    /// outbound golden is the reverse, `source ‖ destination` =
    /// 0000000000002105 ‖ 00000000536f6c4e. Only the 32-byte intent hash
    /// (0x11..) and claimant (0x22..) are shared, so those are the fields to
    /// eyeball across both legs. Do not regenerate from `abi_encode_bytes`; if
    /// the EVM event changes shape, this literal and its eco-routes counterpart
    /// change together.
    #[test]
    fn parse_matches_solidity_abi_encoding() {
        const SOLIDITY_TOPICS: &str =
            "d493dde4de24066db29a754b1e9dc5dedf7084850c4abf76c23046ba9dad6b1d\
            00000000000000000000000000000000000000000000000000000000536f6c4e";
        const SOLIDITY_DATA: &str =
            "0000000000000000000000000000000000000000000000000000000000000020\
            0000000000000000000000000000000000000000000000000000000000000048\
            0000000000002105\
            1111111111111111111111111111111111111111111111111111111111111111\
            2222222222222222222222222222222222222222222222222222222222222222\
            000000000000000000000000000000000000000000000000";
        let mut expected_proofs = 8453u64.to_be_bytes().to_vec();
        expected_proofs.extend_from_slice(&[0x11u8; 32]);
        expected_proofs.extend_from_slice(&[0x22u8; 32]);

        assert_eq!(
            parse_source(&decode_hex(SOLIDITY_TOPICS)).unwrap(),
            1399811150
        );
        assert_eq!(
            decode_encoded_proofs(&decode_hex(SOLIDITY_DATA)).unwrap(),
            expected_proofs
        );
    }

    #[test]
    fn abi_encode_bytes_pads_to_word() {
        goldie::assert_debug!((
            abi_encode_bytes(&[]),
            abi_encode_bytes(&[1, 2, 3]),
            abi_encode_bytes(&[9u8; 32]),
        ));
    }

    #[test]
    fn parse_rejects_wrong_topics_length() {
        let mut t = topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1);
        t.extend_from_slice(&[0u8; 32]);
        let err = parse_source(&t).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidTopicsLength.into());
    }

    #[test]
    fn parse_rejects_wrong_selector() {
        let err = parse_source(&topics([0u8; 32], 1)).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventSignature.into());
    }

    #[test]
    fn parse_rejects_source_wider_than_u64() {
        let mut t = topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1);
        t[32] = 1; // high byte of the second topic word
        let err = parse_source(&t).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidSourceChain.into());
    }

    #[test]
    fn parse_rejects_bad_abi_offset() {
        let mut data = abi_encode_bytes(&[1u8; 8]);
        data[31] = 64;
        let err = decode_encoded_proofs(&data).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn parse_rejects_short_buffer() {
        let data = abi_encode_bytes(&[1u8; 40]);
        let truncated = &data[..data.len() - 1];
        let err = decode_encoded_proofs(truncated).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn parse_rejects_length_beyond_buffer() {
        let mut data = abi_encode_bytes(&[1u8; 8]);
        data[63] = 200;
        let err = decode_encoded_proofs(&data).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn parse_rejects_absurd_length_word() {
        // `u64::MAX` is a well-formed `uint64` word (bytes 32..56 stay zero), so
        // it passes `uint64_word` and reaches the padded-length arithmetic. The
        // `checked_mul` in `abi_decode_bytes` is what turns it into
        // `InvalidEventData`; with a plain `*` the multiply overflows and panics
        // (dev and `[profile.release] overflow-checks = true`) instead.
        let mut data = abi_encode_bytes(&[1u8; 8]);
        data[2 * 32 - 8..2 * 32].copy_from_slice(&u64::MAX.to_be_bytes());
        let err = decode_encoded_proofs(&data).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn parse_rejects_non_uint64_length_word() {
        // A length word with a non-zero byte above the low 8 is not a valid
        // `uint64`; `uint64_word` rejects it before any length arithmetic.
        let mut data = abi_encode_bytes(&[1u8; 8]);
        data[2 * 32 - 9] = 1;
        let err = decode_encoded_proofs(&data).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn evm_address_left_pads() {
        let address = [0xaau8; 20];
        let bytes: [u8; 32] = evm_address_to_bytes32(address).into();
        assert_eq!(bytes[..12], [0u8; 12]);
        assert_eq!(bytes[12..], address);
    }
}
