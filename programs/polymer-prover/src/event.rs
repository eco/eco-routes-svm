//! Decoding of the EVM `IntentFulfilledFromSource(uint64 indexed source, bytes
//! encodedProofs)` event as Polymer returns it: flat 32-byte topics and the
//! ABI-encoded non-indexed data.

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

#[derive(Debug, PartialEq, Eq)]
pub struct IntentFulfilledFromSource {
    /// Chain the intent was published on, i.e. this chain's `CHAIN_ID`.
    pub source: u64,
    /// `ProofData` bytes: 8-byte destination followed by (intent hash, claimant) pairs.
    pub encoded_proofs: Vec<u8>,
}

impl IntentFulfilledFromSource {
    pub fn parse(topics: &[u8], unindexed_data: &[u8]) -> Result<Self> {
        require!(
            topics.len() == TOPICS_LEN,
            PolymerProverError::InvalidTopicsLength
        );
        require!(
            topics[..WORD] == INTENT_FULFILLED_FROM_SOURCE_SELECTOR,
            PolymerProverError::InvalidEventSignature
        );
        let source =
            uint64_word(&topics[WORD..TOPICS_LEN]).ok_or(PolymerProverError::InvalidSourceChain)?;
        let encoded_proofs =
            abi_decode_bytes(unindexed_data).ok_or(PolymerProverError::InvalidEventData)?;

        Ok(Self {
            source,
            encoded_proofs,
        })
    }
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
/// offset is not 32, or the buffer is shorter than the padded payload.
fn abi_decode_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let offset = uint64_word(data.get(..WORD)?)?;
    if offset != WORD as u64 {
        return None;
    }
    let len = usize::try_from(uint64_word(data.get(WORD..2 * WORD)?)?).ok()?;
    let padded_end = 2 * WORD + len.div_ceil(WORD) * WORD;
    if data.len() < padded_end {
        return None;
    }
    Some(data[2 * WORD..2 * WORD + len].to_vec())
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

    #[test]
    fn selector_matches_keccak_of_signature() {
        let expected = keccak(b"IntentFulfilledFromSource(uint64,bytes)").to_bytes();
        assert_eq!(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, expected);
    }

    #[test]
    fn parse_success() {
        let payload = vec![0x11u8; 72];
        let event = IntentFulfilledFromSource::parse(
            &topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1399811150),
            &abi_encode_bytes(&payload),
        )
        .unwrap();

        assert_eq!(event.source, 1399811150);
        assert_eq!(event.encoded_proofs, payload);
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
        let err = IntentFulfilledFromSource::parse(&t, &abi_encode_bytes(&[1u8; 8])).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidTopicsLength.into());
    }

    #[test]
    fn parse_rejects_wrong_selector() {
        let err =
            IntentFulfilledFromSource::parse(&topics([0u8; 32], 1), &abi_encode_bytes(&[1u8; 8]))
                .unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventSignature.into());
    }

    #[test]
    fn parse_rejects_source_wider_than_u64() {
        let mut t = topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1);
        t[32] = 1; // high byte of the second topic word
        let err = IntentFulfilledFromSource::parse(&t, &abi_encode_bytes(&[1u8; 8])).unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidSourceChain.into());
    }

    #[test]
    fn parse_rejects_bad_abi_offset() {
        let mut data = abi_encode_bytes(&[1u8; 8]);
        data[31] = 64;
        let err = IntentFulfilledFromSource::parse(
            &topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1),
            &data,
        )
        .unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn parse_rejects_short_buffer() {
        let data = abi_encode_bytes(&[1u8; 40]);
        let truncated = &data[..data.len() - 1];
        let err = IntentFulfilledFromSource::parse(
            &topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1),
            truncated,
        )
        .unwrap_err();
        assert_eq!(err, PolymerProverError::InvalidEventData.into());
    }

    #[test]
    fn parse_rejects_length_beyond_buffer() {
        let mut data = abi_encode_bytes(&[1u8; 8]);
        data[63] = 200;
        let err = IntentFulfilledFromSource::parse(
            &topics(INTENT_FULFILLED_FROM_SOURCE_SELECTOR, 1),
            &data,
        )
        .unwrap_err();
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
