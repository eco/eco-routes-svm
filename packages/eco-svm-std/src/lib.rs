use anchor_lang::prelude::*;
use derive_more::Deref;
use derive_new::new;

pub const PROOF_SEED: &[u8] = b"proof";

#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Deref, Clone, Copy, Debug, PartialEq, Eq,
)]
pub struct Bytes32([u8; 32]);

impl From<[u8; 32]> for Bytes32 {
    fn from(bytes: [u8; 32]) -> Self {
        Bytes32(bytes)
    }
}

impl From<Bytes32> for [u8; 32] {
    fn from(bytes: Bytes32) -> Self {
        bytes.0
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, new)]
pub struct Proof {
    pub destination_chain: Bytes32,
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
