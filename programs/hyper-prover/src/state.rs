use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;

use crate::instructions::HyperProverError;

pub const DISPATCHER_SEED: &[u8] = b"dispatcher";
pub const CONFIG_SEED: &[u8] = b"config";
pub const PDA_PAYER_SEED: &[u8] = b"pda_payer";
const MAX_WHITELIST_LEN: usize = 20;

#[account]
#[derive(InitSpace)]
pub struct ProofAccount(pub eco_svm_std::prover::Proof);

impl AccountExt for ProofAccount {}

impl From<eco_svm_std::prover::Proof> for ProofAccount {
    fn from(proof: eco_svm_std::prover::Proof) -> Self {
        Self(proof)
    }
}

pub fn dispatcher_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[DISPATCHER_SEED], &crate::ID)
}

pub fn pda_payer_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[PDA_PAYER_SEED], &crate::ID)
}

#[account]
#[derive(InitSpace)]
pub struct Config {
    #[max_len(MAX_WHITELIST_LEN)]
    pub whitelisted_senders: Vec<Bytes32>,
}

impl Config {
    pub fn new(whitelisted_senders: Vec<Bytes32>) -> Result<Self> {
        if whitelisted_senders.len() > MAX_WHITELIST_LEN {
            return Err(HyperProverError::TooManyWhitelistedSenders.into());
        }

        Ok(Self {
            whitelisted_senders,
        })
    }

    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID)
    }

    pub fn is_whitelisted(&self, sender: &Bytes32) -> bool {
        self.whitelisted_senders.contains(sender)
    }
}

impl AccountExt for Config {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatcher_pda_deterministic() {
        goldie::assert_json!(dispatcher_pda());
    }

    #[test]
    fn pda_payer_pda_deterministic() {
        goldie::assert_json!(pda_payer_pda());
    }

    #[test]
    fn config_pda_deterministic() {
        goldie::assert_json!(Config::pda());
    }

    #[test]
    fn config_new_success() {
        let whitelisted_senders = vec![[1u8; 32].into(), [2u8; 32].into()];
        let config = Config::new(whitelisted_senders.clone()).unwrap();

        assert_eq!(config.whitelisted_senders, whitelisted_senders);
    }

    #[test]
    fn config_new_too_many_senders() {
        let whitelisted_senders = vec![[0u8; 32].into(); MAX_WHITELIST_LEN + 1];

        assert!(Config::new(whitelisted_senders).is_err());
    }

    #[test]
    fn config_is_whitelisted() {
        let sender1: Bytes32 = [1u8; 32].into();
        let sender2: Bytes32 = [2u8; 32].into();
        let sender3: Bytes32 = [3u8; 32].into();

        let config = Config::new(vec![sender1, sender2]).unwrap();

        assert!(config.is_whitelisted(&sender1));
        assert!(config.is_whitelisted(&sender2));
        assert!(!config.is_whitelisted(&sender3));
    }
}
