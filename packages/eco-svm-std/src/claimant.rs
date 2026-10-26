use anchor_lang::prelude::*;

use crate::CANCELLED;

pub fn cancelled() -> Pubkey {
    Pubkey::new_from_array(CANCELLED.into())
}

pub fn is_cancelled(claimant: &Pubkey) -> bool {
    CANCELLED == *claimant
}

pub fn is_payable(claimant: &Pubkey) -> bool {
    *claimant != Pubkey::default() && !is_cancelled(claimant)
}
