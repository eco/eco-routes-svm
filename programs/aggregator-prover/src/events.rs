use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

// Declared locally so Anchor exports the shared prover event ABI in this program's IDL.
#[event]
pub struct IntentProven {
    pub intent_hash: Bytes32,
    pub claimant: Pubkey,
    pub destination: u64,
}
