use anchor_lang::prelude::*;
use derive_more::Deref;
use derive_new::new;

pub mod account;

const PROVER_PREFIX: &str = "Prover";
pub const PROOF_SEED: &[u8] = b"proof";
pub const CHAIN_ID: u64 = 1399811149;

pub fn is_prover(program_id: &Pubkey) -> bool {
    program_id.to_string().starts_with(PROVER_PREFIX)
}

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

impl PartialEq<Pubkey> for Bytes32 {
    fn eq(&self, pubkey: &Pubkey) -> bool {
        self.0 == pubkey.to_bytes()
    }
}

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

#[cfg(test)]
mod tests {
    use anchor_lang::system_program;

    use super::*;

    #[test]
    fn proof_pda_deterministic() {
        let intent_hash = [42u8; 32].into();
        let prover = Pubkey::new_from_array([123u8; 32]);

        goldie::assert_debug!(Proof::pda(&intent_hash, &prover));
    }

    #[test]
    fn is_prover_true() {
        assert!(is_prover(
            &"Prover1111111111111111111111111111111111111"
                .parse()
                .unwrap()
        ));
    }

    #[test]
    fn is_prover_false() {
        assert!(!is_prover(&system_program::ID));
    }
}
