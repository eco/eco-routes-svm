use anchor_lang::prelude::*;
use derive_new::new;

use crate::{claimant, Bytes32};

pub mod cpi;

pub const PROOF_SEED: &[u8] = b"proof";
pub const PROVE_DISCRIMINATOR: [u8; 8] = [52, 246, 26, 161, 211, 170, 86, 215];
pub const CLOSE_PROOF_DISCRIMINATOR: [u8; 8] = [64, 76, 168, 8, 126, 109, 164, 179];
pub const GET_PROOF_DISCRIMINATOR: [u8; 8] = [4, 113, 134, 136, 211, 190, 9, 99];
const PROOF_ACCOUNT_DISCRIMINATOR: [u8; 8] = [54, 244, 192, 233, 218, 58, 44, 242];

#[derive(AnchorSerialize, AnchorDeserialize, Clone, new)]
pub struct GetProofArgs {
    pub intent_hash: Bytes32,
    pub data: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, new)]
pub struct CloseProofArgs {
    pub intent_hash: Bytes32,
    pub data: Vec<u8>,
}

#[error_code]
pub enum ProverError {
    InvalidProof,
    InvalidReturnData,
}

#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Default, new, Debug, PartialEq, Eq,
)]
pub struct Proof {
    pub destination: u64,
    pub claimant: Pubkey,
}

impl Proof {
    pub fn pda(intent_hash: &Bytes32, prover: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[PROOF_SEED, intent_hash.as_ref()], prover)
    }

    pub fn is_cancelled(&self) -> bool {
        claimant::is_cancelled(&self.claimant)
    }

    pub fn is_payable_to(&self, claimant: &Pubkey) -> bool {
        self.claimant == *claimant && claimant::is_payable(&self.claimant)
    }

    pub fn get(
        account: &AccountInfo,
        prover: &Pubkey,
        intent_hash: &Bytes32,
    ) -> Result<Option<Self>> {
        require_keys_eq!(
            account.key(),
            Self::pda(intent_hash, prover).0,
            ProverError::InvalidProof
        );

        let data = account.try_borrow_data()?;
        if account.owner != prover || !data.starts_with(&PROOF_ACCOUNT_DISCRIMINATOR) {
            return Ok(None);
        }

        Ok(Self::try_from_slice(&data[8..])
            .ok()
            .filter(|proof| proof.claimant != Pubkey::default()))
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

#[derive(new, AnchorSerialize, AnchorDeserialize, PartialEq, Clone, Debug)]
pub struct ProofData {
    pub destination: u64,
    pub intent_hashes_claimants: Vec<IntentHashClaimant>,
}

impl ProofData {
    pub fn to_bytes(self) -> Vec<u8> {
        let Self {
            destination,
            intent_hashes_claimants,
        } = self;

        destination
            .to_be_bytes()
            .into_iter()
            .chain(intent_hashes_claimants.into_iter().flat_map(
                |IntentHashClaimant {
                     intent_hash,
                     claimant,
                 }| { intent_hash.into_iter().chain(claimant) },
            ))
            .collect()
    }

    // `clippy::manual_is_multiple_of` (new in 1.97) wants `is_multiple_of` here, but that
    // method does not exist in the rustc the Solana platform-tools ship, so the on-chain
    // build cannot use it. Keep the `%` form until the platform-tools rustc catches up.
    #[allow(clippy::manual_is_multiple_of)]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        require!(
            bytes.len() >= 8,
            anchor_lang::error::ErrorCode::InstructionDidNotDeserialize
        );
        require!(
            (bytes.len() - 8) % 64 == 0,
            anchor_lang::error::ErrorCode::InstructionDidNotDeserialize
        );

        let (destination, intent_hashes_claimants) = bytes.split_at(8);

        let destination = u64::from_be_bytes(destination.try_into().expect("slice is 8 bytes"));
        let intent_hashes_claimants = intent_hashes_claimants
            .chunks_exact(64)
            .map(|chunk| {
                let intent_hash = <[u8; 32]>::try_from(&chunk[..32])
                    .expect("slice is 32 bytes")
                    .into();
                let claimant = <[u8; 32]>::try_from(&chunk[32..])
                    .expect("slice is 32 bytes")
                    .into();

                IntentHashClaimant::new(intent_hash, claimant)
            })
            .collect();

        Ok(ProofData::new(destination, intent_hashes_claimants))
    }
}

#[derive(new, AnchorSerialize, AnchorDeserialize, PartialEq, Clone, Debug)]
pub struct IntentHashClaimant {
    pub intent_hash: Bytes32,
    pub claimant: Bytes32,
}

#[derive(AnchorSerialize, AnchorDeserialize, new)]
pub struct ProveArgs {
    pub domain_id: u64,
    pub proof_data: ProofData,
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
    fn proof_distinguishes_payable_cancelled_and_zero_claimants() {
        let claimant = Pubkey::new_unique();
        let proof = Proof::new(10, claimant);
        assert!(proof.is_payable_to(&claimant));
        assert!(!proof.is_payable_to(&Pubkey::new_unique()));
        assert!(!proof.is_cancelled());

        let cancelled = Proof::new(10, claimant::cancelled());
        assert!(cancelled.is_cancelled());
        assert!(!cancelled.is_payable_to(&cancelled.claimant));

        let zero = Proof::new(10, Pubkey::default());
        assert!(!zero.is_cancelled());
        assert!(!zero.is_payable_to(&zero.claimant));
    }

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
        let intent_hashes_claimants =
            ProofData::new(8u64, vec![IntentHashClaimant::new(intent_hash, claimant)]);

        goldie::assert_debug!(intent_hashes_claimants.to_bytes());
    }

    #[test]
    fn intent_hashes_claimants_to_bytes_multiple() {
        let intent_hashes_claimants = ProofData::new(
            8u64,
            vec![
                IntentHashClaimant::new(Bytes32::from([0xaa; 32]), Bytes32::from([0xbb; 32])),
                IntentHashClaimant::new(Bytes32::from([0xcc; 32]), Bytes32::from([0xdd; 32])),
                IntentHashClaimant::new(Bytes32::from([0xee; 32]), Bytes32::from([0xff; 32])),
            ],
        );

        goldie::assert_debug!(intent_hashes_claimants.to_bytes());
    }

    #[test]
    fn intent_hashes_claimants_to_bytes_empty() {
        let intent_hashes_claimants = ProofData::new(8u64, vec![]);

        goldie::assert_debug!(intent_hashes_claimants.to_bytes());
    }

    #[test]
    fn intent_hashes_claimants_from_bytes_single() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8u64.to_be_bytes()); // destination
        bytes.extend_from_slice(&[0x11; 32]); // intent_hash
        bytes.extend_from_slice(&[0x22; 32]); // claimant

        let result = ProofData::from_bytes(&bytes).unwrap();
        goldie::assert_debug!(result);
    }

    #[test]
    fn intent_hashes_claimants_from_bytes_multiple() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8u64.to_be_bytes()); // destination
        bytes.extend_from_slice(&[0xaa; 32]); // intent_hash 1
        bytes.extend_from_slice(&[0xbb; 32]); // claimant 1
        bytes.extend_from_slice(&[0xcc; 32]); // intent_hash 2
        bytes.extend_from_slice(&[0xdd; 32]); // claimant 2
        bytes.extend_from_slice(&[0xee; 32]); // intent_hash 3
        bytes.extend_from_slice(&[0xff; 32]); // claimant 3

        let result = ProofData::from_bytes(&bytes).unwrap();
        goldie::assert_debug!(result);
    }

    #[test]
    fn intent_hashes_claimants_from_bytes_empty() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8u64.to_be_bytes()); // destination only

        let result = ProofData::from_bytes(&bytes).unwrap();
        goldie::assert_debug!(result);
    }

    #[test]
    fn intent_hashes_claimants_from_bytes_invalid_length() {
        let bytes = vec![0u8; 63];

        let result = ProofData::from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn intent_hashes_claimants_roundtrip() {
        let expected = ProofData::new(
            8u64,
            vec![
                IntentHashClaimant::new(Bytes32::from([0x01; 32]), Bytes32::from([0x02; 32])),
                IntentHashClaimant::new(Bytes32::from([0x03; 32]), Bytes32::from([0x04; 32])),
            ],
        );

        let bytes = expected.clone().to_bytes();
        let actual = ProofData::from_bytes(&bytes).unwrap();

        assert_eq!(expected, actual);
    }
}
