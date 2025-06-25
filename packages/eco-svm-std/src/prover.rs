use anchor_lang::prelude::*;
use derive_new::new;

use crate::Bytes32;

pub const PROOF_SEED: &[u8] = b"proof";
pub const PROVE_DISCRIMINATOR: [u8; 8] = [52, 246, 26, 161, 211, 170, 86, 215];

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, new)]
pub struct Proof {
    pub destination_chain: u64,
    pub claimant: Pubkey,
}

impl Proof {
    pub fn pda(intent_hash: &Bytes32, prover: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[PROOF_SEED, intent_hash.as_ref()], prover)
    }

    pub fn try_from_account_info(account: &AccountInfo<'_>) -> Result<Option<Self>> {
        match account.data_is_empty() {
            true => Ok(None),
            false => Ok(Some(Proof::deserialize(&mut &account.data.borrow()[8..])?)),
        }
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, new)]
pub struct ProveArgs {
    pub source_chain: u64,
    pub intent_hash: Bytes32,
    pub data: Vec<u8>,
    pub claimant: Bytes32,
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
