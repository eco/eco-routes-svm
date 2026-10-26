use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;

pub const CONFIG_SEED: &[u8] = b"config";
pub const MAX_PROVERS: usize = 8;

#[account]
#[derive(InitSpace)]
pub struct Config {
    #[max_len(MAX_PROVERS)]
    pub provers: Vec<Pubkey>,
}

impl Config {
    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID)
    }
}

impl AccountExt for Config {}
