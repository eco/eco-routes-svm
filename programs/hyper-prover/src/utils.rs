use anchor_lang::prelude::{Pubkey, *};
use eco_svm_std::Bytes32;

use crate::instructions::HyperProverError;

pub fn claimant_and_intent_hash(payload: Vec<u8>) -> Result<(Pubkey, Bytes32)> {
    if payload.len() != 64 {
        return Err(HyperProverError::InvalidData.into());
    }

    let claimant = Pubkey::new_from_array(payload[0..32].try_into().expect("slice is 32 bytes"));
    let intent_hash = <[u8; 32]>::try_from(&payload[32..])
        .expect("slice is 32 bytes")
        .into();

    Ok((claimant, intent_hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claimant_and_intent_hash_success() {
        let claimant = Pubkey::new_unique();
        let intent_hash: Bytes32 = [42u8; 32].into();
        let payload = claimant
            .as_array()
            .iter()
            .copied()
            .chain(intent_hash.to_vec())
            .collect();

        goldie::assert_debug!(claimant_and_intent_hash(payload).unwrap());
    }

    #[test]
    fn claimant_and_intent_hash_invalid_length() {
        let payload = vec![0u8; 63];

        assert!(claimant_and_intent_hash(payload).is_err());
    }
}
