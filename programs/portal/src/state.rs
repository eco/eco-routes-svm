use anchor_lang::prelude::*;
use derive_new::new;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;

pub const VAULT_SEED: &[u8] = b"vault";
pub const CLAIMED_MARKER_SEED: &[u8] = b"claimed_marker";
pub const FULFILL_MARKER_SEED: &[u8] = b"fulfill_marker";
pub const EXECUTOR_SEED: &[u8] = b"executor";

pub fn vault_pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_SEED, intent_hash.as_ref()], &crate::ID)
}

pub fn executor_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[EXECUTOR_SEED], &crate::ID)
}

#[account]
#[derive(InitSpace, Default, Debug)]
pub struct WithdrawnMarker {}

impl AccountExt for WithdrawnMarker {}

impl WithdrawnMarker {
    pub fn pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CLAIMED_MARKER_SEED, intent_hash.as_ref()], &crate::ID)
    }

    pub fn min_balance(rent: Rent) -> u64 {
        rent.minimum_balance(8 + Self::INIT_SPACE)
    }
}

#[account]
#[derive(InitSpace, Debug, PartialEq, new)]
pub struct FulfillMarker {
    pub claimant: Pubkey,
    pub bump: u8,
}

impl AccountExt for FulfillMarker {}

impl FulfillMarker {
    pub fn pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[FULFILL_MARKER_SEED, intent_hash.as_ref()], &crate::ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types;

    #[test]
    fn vault_pda_deterministic() {
        let destination_chain = 1000;
        let route_hash = [6u8; 32].into();
        let reward_hash = [8u8; 32].into();

        goldie::assert_json!(vault_pda(&types::intent_hash(
            destination_chain,
            &route_hash,
            &reward_hash,
        )));
    }

    #[test]
    fn withdrawn_marker_pda_deterministic() {
        let destination_chain = 1000;
        let route_hash = [6u8; 32].into();
        let reward_hash = [8u8; 32].into();

        goldie::assert_json!(WithdrawnMarker::pda(&types::intent_hash(
            destination_chain,
            &route_hash,
            &reward_hash,
        )));
    }

    #[test]
    fn withdrawn_marker_min_balance_deterministic() {
        let rent = Rent {
            lamports_per_byte_year: 3480,
            exemption_threshold: 2.0,
            burn_percent: 50,
        };

        goldie::assert_json!(WithdrawnMarker::min_balance(rent));
    }

    #[test]
    fn executor_pda_deterministic() {
        goldie::assert_json!(executor_pda());
    }

    #[test]
    fn fulfill_marker_pda_deterministic() {
        let destination_chain = 1000;
        let route_hash = [6u8; 32].into();
        let reward_hash = [8u8; 32].into();

        goldie::assert_json!(FulfillMarker::pda(&types::intent_hash(
            destination_chain,
            &route_hash,
            &reward_hash,
        )));
    }
}
