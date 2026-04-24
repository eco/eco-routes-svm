use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
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

/// CPIs a prover program's `prove` instruction for a single intent.
///
/// Generic over any prover that follows the standard `prove(ProveArgs)` shape
/// (local-prover, hyper-prover, etc.). `caller` is signed via `caller_seeds`,
/// so PDAs that the prover accepts as authorized callers (e.g. a dispatcher
/// or flash-vault PDA) can invoke this helper.
#[allow(clippy::too_many_arguments)]
pub fn prove<'info>(
    prover_program: &AccountInfo<'info>,
    caller: &AccountInfo<'info>,
    caller_seeds: &[&[u8]],
    payer: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    event_authority: &AccountInfo<'info>,
    proof: &AccountInfo<'info>,
    args: ProveArgs,
) -> Result<()> {
    let mut data = PROVE_DISCRIMINATOR.to_vec();
    args.serialize(&mut data)?;

    let accounts = vec![
        AccountMeta::new_readonly(caller.key(), true),
        AccountMeta::new(payer.key(), true),
        AccountMeta::new_readonly(system_program.key(), false),
        AccountMeta::new_readonly(event_authority.key(), false),
        AccountMeta::new_readonly(prover_program.key(), false),
        AccountMeta::new(proof.key(), false),
    ];

    let infos = [
        caller.to_account_info(),
        payer.to_account_info(),
        system_program.to_account_info(),
        event_authority.to_account_info(),
        prover_program.to_account_info(),
        proof.to_account_info(),
    ];

    let ix = Instruction {
        program_id: prover_program.key(),
        accounts,
        data,
    };

    invoke_signed(&ix, &infos, &[caller_seeds]).map_err(Into::into)
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
