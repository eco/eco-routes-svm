use anchor_lang::prelude::*;
use derive_new::new;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;

use crate::instructions::PortalError;

pub const VAULT_SEED: &[u8] = b"vault";
pub const CLAIMED_MARKER_SEED: &[u8] = b"claimed_marker";
pub const FULFILL_MARKER_SEED: &[u8] = b"fulfill_marker";
pub const EXECUTOR_SEED: &[u8] = b"executor";
pub const DISPATCHER_SEED: &[u8] = b"dispatcher";
pub const PROOF_CLOSER_SEED: &[u8] = b"proof_closer";

pub fn vault_pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_SEED, intent_hash.as_ref()], &crate::ID)
}

pub fn executor_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[EXECUTOR_SEED], &crate::ID)
}

/// Per-prover authority: `prove` signs this into the caller-chosen prover, and
/// each prover accepts only `dispatcher_pda(&its_own_id)`. The prover binding is
/// a security boundary — keep it seeded by the prover and do not collapse it to a
/// single shared PDA.
pub fn dispatcher_pda(prover: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[DISPATCHER_SEED, prover.as_ref()], &crate::ID)
}

/// Per-prover authority: `withdraw` signs this into the caller-chosen prover's
/// `close_proof` CPI, and each prover accepts only `proof_closer_pda(&its_own_id)`.
/// The prover binding is a security boundary — keep it seeded by the prover and
/// do not collapse it to a single shared PDA.
pub fn proof_closer_pda(prover: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[PROOF_CLOSER_SEED, prover.as_ref()], &crate::ID)
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

/// The whole field order is on-chain ABI: `prove` deserializes the full struct
/// (`fulfillment_claimant`), so reordering breaks it, not just moving `claimant`.
/// `fulfill_marker_layout_deterministic` pins the encoding.
///
/// Growing or reordering it is only safe because the portal is redeployed under
/// a new program ID rather than upgraded in place: markers are PDAs of the
/// program, so a redeploy starts on a disjoint namespace and no account written
/// under an older layout is ever read back. Under an in-place upgrade the same
/// change would strand every fulfilled-but-unproven intent — `prove`'s
/// `try_deserialize` of a shorter account fails with `InvalidFulfillMarker`,
/// and the claimant it holds has no other source.
///
/// `payer` is the sole authority allowed to close the marker and reclaim its
/// rent; `deadline` is `route.deadline`, which gates that close.
#[account]
#[derive(InitSpace, Debug, PartialEq, new)]
pub struct FulfillMarker {
    pub claimant: Bytes32,
    pub payer: Pubkey,
    pub deadline: u64,
    pub bump: u8,
}

impl AccountExt for FulfillMarker {}

impl FulfillMarker {
    pub fn pda(intent_hash: &Bytes32) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[FULFILL_MARKER_SEED, intent_hash.as_ref()], &crate::ID)
    }
}

/// What `close_fulfill_marker` leaves at a [`FulfillMarker`]'s PDA.
///
/// Keeping the PDA occupied is what keeps `cancel` sound: a deleted marker is
/// indistinguishable from "never fulfilled", so `cancel` could otherwise
/// succeed on a fulfilled intent after its solver reclaimed rent. It also
/// keeps `fulfill` failing, and it keeps `claimant`, so the intent stays
/// provable after the close.
#[account]
#[derive(InitSpace, Debug, PartialEq, new)]
pub struct FulfillTombstone {
    pub claimant: Bytes32,
}

/// Claimant recorded for an intent on this chain, read from its live
/// [`FulfillMarker`] or from the [`FulfillTombstone`] it was closed to.
pub fn fulfillment_claimant(account: &AccountInfo) -> Result<Bytes32> {
    let data = account.try_borrow_data()?;

    if let Ok(marker) = FulfillMarker::try_deserialize(&mut &data[..]) {
        return Ok(marker.claimant);
    }

    FulfillTombstone::try_deserialize(&mut &data[..])
        .map(|tombstone| tombstone.claimant)
        .map_err(|_| PortalError::InvalidFulfillMarker.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types;

    #[test]
    fn vault_pda_deterministic() {
        let destination = 1000;
        let route_hash = [6u8; 32].into();
        let reward_hash = [8u8; 32].into();

        goldie::assert_json!(vault_pda(&types::intent_hash(
            destination,
            &route_hash,
            &reward_hash,
        )));
    }

    #[test]
    fn withdrawn_marker_pda_deterministic() {
        let destination = 1000;
        let route_hash = [6u8; 32].into();
        let reward_hash = [8u8; 32].into();

        goldie::assert_json!(WithdrawnMarker::pda(&types::intent_hash(
            destination,
            &route_hash,
            &reward_hash,
        )));
    }

    #[test]
    #[allow(deprecated)] // Rent fields are deprecated but still the way to build a fixed test fixture
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
    fn dispatcher_pda_deterministic() {
        goldie::assert_json!(dispatcher_pda(&Pubkey::new_from_array([9u8; 32])));
    }

    #[test]
    fn fulfill_marker_pda_deterministic() {
        let destination = 1000;
        let route_hash = [6u8; 32].into();
        let reward_hash = [8u8; 32].into();

        goldie::assert_json!(FulfillMarker::pda(&types::intent_hash(
            destination,
            &route_hash,
            &reward_hash,
        )));
    }

    /// Pins the account size *and* the field order — `prove` deserializes the
    /// whole struct, so a reorder is an ABI break that a size-only assertion
    /// would not catch. Each field gets a distinct byte pattern.
    #[test]
    fn fulfill_marker_layout_deterministic() {
        let marker = FulfillMarker::new(
            [1u8; 32].into(),
            Pubkey::new_from_array([2u8; 32]),
            0x0304050607080910,
            11,
        );

        goldie::assert_json!((
            8 + FulfillMarker::INIT_SPACE,
            borsh::to_vec(&marker).unwrap()
        ));
    }

    /// Pins size, discriminator and field order: `prove` reads `claimant` out
    /// of a tombstone, so all three are ABI.
    #[test]
    fn fulfill_tombstone_layout_deterministic() {
        let tombstone = FulfillTombstone::new([1u8; 32].into());

        goldie::assert_json!((
            8 + FulfillTombstone::INIT_SPACE,
            FulfillTombstone::DISCRIMINATOR,
            borsh::to_vec(&tombstone).unwrap()
        ));
    }

    #[test]
    fn proof_closer_pda_deterministic() {
        goldie::assert_json!(proof_closer_pda(&Pubkey::new_from_array([9u8; 32])));
    }
}
