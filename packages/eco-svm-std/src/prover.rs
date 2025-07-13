use anchor_lang::prelude::*;
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

#[derive(AnchorSerialize, AnchorDeserialize, new)]
pub struct ProveArgs {
    pub source: u64,
    pub intent_hash: Bytes32,
    pub data: Vec<u8>,
    pub claimant: Bytes32,
}

#[event]
#[derive(new)]
pub struct IntentProven {
    intent_hash: Bytes32,
    source: u64,
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
}
