use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;

use crate::instructions::PolymerProverError;

pub const CONFIG_SEED: &[u8] = b"config";
/// Largest emitter whitelist `init` can write. `Config::INIT_SPACE` is
/// `4 + 32 * MAX_WHITELIST_LEN` and `AccountExt::init` allocates exactly
/// `8 + INIT_SPACE`, so a full whitelist serializes byte-for-byte into the
/// account; raising this constant without the `#[max_len]` below moving with
/// it makes `init` fail at serialization. `init` is one-shot and
/// unauthenticated, so that failure burns the program ID at rollout.
/// Pinned by `init_polymer_prover_at_max_whitelist_success`.
pub const MAX_WHITELIST_LEN: usize = 20;

#[account]
#[derive(InitSpace)]
pub struct ProofAccount(pub eco_svm_std::prover::Proof);

impl AccountExt for ProofAccount {}

impl From<eco_svm_std::prover::Proof> for ProofAccount {
    fn from(proof: eco_svm_std::prover::Proof) -> Self {
        Self(proof)
    }
}

/// Whitelist of EVM `PolymerProver` contracts whose `IntentFulfilledFromSource`
/// events this program accepts. Each entry is a 20-byte EVM address left-padded
/// to 32 bytes.
#[account]
#[derive(InitSpace)]
pub struct Config {
    #[max_len(MAX_WHITELIST_LEN)]
    pub whitelisted_emitters: Vec<Bytes32>,
}

impl Config {
    pub fn new(whitelisted_emitters: Vec<Bytes32>) -> Result<Self> {
        if whitelisted_emitters.len() > MAX_WHITELIST_LEN {
            return Err(PolymerProverError::TooManyWhitelistedEmitters.into());
        }

        Ok(Self {
            whitelisted_emitters,
        })
    }

    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID)
    }

    pub fn is_whitelisted(&self, emitter: &Bytes32) -> bool {
        self.whitelisted_emitters.contains(emitter)
    }
}

impl AccountExt for Config {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_pda_deterministic() {
        goldie::assert_json!(Config::pda());
    }

    /// The portal authority `prove` accepts, scoped to this program's ID. A seed
    /// change here silently locks out portal — pin the address, not just the
    /// derivation.
    ///
    /// The golden is a byte array nobody can read in a diff, so the base58 form
    /// is asserted inline too: a seed change then fails with a readable address.
    #[test]
    fn accepted_prove_caller_authority_deterministic() {
        let (pda, _) = portal::state::dispatcher_pda(&crate::ID);
        assert_eq!(
            pda.to_string(),
            "ACL8p7ice1LimrdnB9355y54dS5gmXi5Zm675bybhU7S"
        );
        goldie::assert_json!(portal::state::dispatcher_pda(&crate::ID));
    }

    /// The portal authority `close_proof` accepts, scoped to this program's ID.
    #[test]
    fn accepted_proof_closer_authority_deterministic() {
        let (pda, _) = portal::state::proof_closer_pda(&crate::ID);
        assert_eq!(
            pda.to_string(),
            "93EzFXtxNJataBPhTEbZv21aPmr5uZwSCtRqgN7iV8SC"
        );
        goldie::assert_json!(portal::state::proof_closer_pda(&crate::ID));
    }

    #[test]
    fn config_new_success() {
        let emitters = vec![[1u8; 32].into(), [2u8; 32].into()];
        let config = Config::new(emitters.clone()).unwrap();

        assert_eq!(config.whitelisted_emitters, emitters);
    }

    #[test]
    fn config_new_too_many_emitters() {
        let emitters = vec![[0u8; 32].into(); MAX_WHITELIST_LEN + 1];

        // `Config` derives no `Debug`, so unwrap the error side directly.
        assert_eq!(
            Config::new(emitters).err().unwrap(),
            PolymerProverError::TooManyWhitelistedEmitters.into()
        );
    }

    #[test]
    fn config_new_accepts_max_emitters() {
        let emitters = vec![[0u8; 32].into(); MAX_WHITELIST_LEN];

        assert!(Config::new(emitters).is_ok());
    }

    #[test]
    fn config_is_whitelisted() {
        let emitter1: Bytes32 = [1u8; 32].into();
        let emitter2: Bytes32 = [2u8; 32].into();
        let emitter3: Bytes32 = [3u8; 32].into();

        let config = Config::new(vec![emitter1, emitter2]).unwrap();

        assert!(config.is_whitelisted(&emitter1));
        assert!(config.is_whitelisted(&emitter2));
        assert!(!config.is_whitelisted(&emitter3));
    }
}
