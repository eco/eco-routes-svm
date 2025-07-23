use anchor_lang::prelude::*;
use derive_more::Deref;
use derive_new::new;

use crate::Bytes32;

pub const PROOF_SEED: &[u8] = b"proof";
pub const PROVE_DISCRIMINATOR: [u8; 8] = [52, 246, 26, 161, 211, 170, 86, 215];
pub const CLOSE_PROOF_DISCRIMINATOR: [u8; 8] = [64, 76, 168, 8, 126, 109, 164, 179];

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Default, new, Debug)]
pub struct Proof {
    pub destination: u64,
    pub claimant: Pubkey,
}

impl Proof {
    pub fn pda(intent_hash: &Bytes32, prover: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[PROOF_SEED, intent_hash.as_ref()], prover)
    }

    pub fn try_from_account_info(account: &AccountInfo<'_>) -> Result<Option<Self>> {
        account
            .data
            .borrow()
            .get(8..)
            .map(Self::try_from_slice)
            .transpose()
            .map_err(Into::into)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Deref, PartialEq, Clone, Debug)]
pub struct IntentHashesClaimants(Vec<(Bytes32, Bytes32)>);

impl From<Vec<(Bytes32, Bytes32)>> for IntentHashesClaimants {
    fn from(value: Vec<(Bytes32, Bytes32)>) -> Self {
        Self(value)
    }
}

impl From<IntentHashesClaimants> for Vec<(Bytes32, Bytes32)> {
    fn from(value: IntentHashesClaimants) -> Self {
        value.0
    }
}

impl IntentHashesClaimants {
    pub fn to_bytes(self) -> Vec<u8> {
        self.0
            .into_iter()
            .flat_map(|(intent_hash, claimant)| intent_hash.into_iter().chain(claimant))
            .collect()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        require!(
            bytes.len() % 64 == 0,
            anchor_lang::error::ErrorCode::AccountDidNotDeserialize
        );

        Ok(bytes
            .chunks_exact(64)
            .map(|chunk| {
                let intent_hash = <[u8; 32]>::try_from(&chunk[..32])
                    .expect("slice is 32 bytes")
                    .into();
                let claimant = <[u8; 32]>::try_from(&chunk[32..])
                    .expect("slice is 32 bytes")
                    .into();

                (intent_hash, claimant)
            })
            .collect::<Vec<(Bytes32, Bytes32)>>()
            .into())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, new)]
pub struct ProveArgs {
    pub source: u64,
    pub intent_hashes_claimants: IntentHashesClaimants,
    pub data: Vec<u8>,
}

#[event]
#[derive(new)]
pub struct IntentProven {
    intent_hash: Bytes32,
    claimant: Pubkey,
    destination: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_pda_deterministic() {
        let intent_hash = [42u8; 32].into();
        let prover = Pubkey::new_from_array([123u8; 32]);

        goldie::assert_debug!(Proof::pda(&intent_hash, &prover));
    }

    #[test]
    fn intent_hashes_claimants_to_bytes_single() {
        let intent_hash: Bytes32 = Bytes32::from([0x11; 32]);
        let claimant: Bytes32 = Bytes32::from([0x22; 32]);
        let intent_hashes_claimants = IntentHashesClaimants::from(vec![(intent_hash, claimant)]);

        goldie::assert_debug!(intent_hashes_claimants.to_bytes());
    }

    #[test]
    fn intent_hashes_claimants_to_bytes_multiple() {
        let intent_hashes_claimants = IntentHashesClaimants::from(vec![
            (Bytes32::from([0xaa; 32]), Bytes32::from([0xbb; 32])),
            (Bytes32::from([0xcc; 32]), Bytes32::from([0xdd; 32])),
            (Bytes32::from([0xee; 32]), Bytes32::from([0xff; 32])),
        ]);

        goldie::assert_debug!(intent_hashes_claimants.to_bytes());
    }

    #[test]
    fn intent_hashes_claimants_to_bytes_empty() {
        let intent_hashes_claimants = IntentHashesClaimants::from(vec![]);

        goldie::assert_debug!(intent_hashes_claimants.to_bytes());
    }

    #[test]
    fn intent_hashes_claimants_from_bytes_single() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x11; 32]);
        bytes.extend_from_slice(&[0x22; 32]);

        let result = IntentHashesClaimants::from_bytes(&bytes).unwrap();
        goldie::assert_debug!(result);
    }

    #[test]
    fn intent_hashes_claimants_from_bytes_multiple() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xaa; 32]);
        bytes.extend_from_slice(&[0xbb; 32]);
        bytes.extend_from_slice(&[0xcc; 32]);
        bytes.extend_from_slice(&[0xdd; 32]);
        bytes.extend_from_slice(&[0xee; 32]);
        bytes.extend_from_slice(&[0xff; 32]);

        let result = IntentHashesClaimants::from_bytes(&bytes).unwrap();
        goldie::assert_debug!(result);
    }

    #[test]
    fn intent_hashes_claimants_from_bytes_empty() {
        let bytes = Vec::new();

        let result = IntentHashesClaimants::from_bytes(&bytes).unwrap();
        goldie::assert_debug!(result);
    }

    #[test]
    fn intent_hashes_claimants_from_bytes_invalid_length() {
        let bytes = vec![0u8; 63];

        let result = IntentHashesClaimants::from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn intent_hashes_claimants_roundtrip() {
        let expected = IntentHashesClaimants::from(vec![
            (Bytes32::from([0x01; 32]), Bytes32::from([0x02; 32])),
            (Bytes32::from([0x03; 32]), Bytes32::from([0x04; 32])),
        ]);

        let bytes = expected.clone().to_bytes();
        let actual = IntentHashesClaimants::from_bytes(&bytes).unwrap();

        assert_eq!(expected, actual);
    }
}
