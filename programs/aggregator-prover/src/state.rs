use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::Proof;

pub const CONFIG_SEED: &[u8] = b"config";
pub const MAX_MEMBERS: usize = 8;

#[account]
#[derive(InitSpace)]
pub struct Config {
    #[max_len(MAX_MEMBERS)]
    pub members: Vec<Pubkey>,
}

impl Config {
    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID)
    }
}

impl AccountExt for Config {}

#[account]
#[derive(InitSpace)]
pub struct ProofAccount(pub Proof);

impl AccountExt for ProofAccount {}
