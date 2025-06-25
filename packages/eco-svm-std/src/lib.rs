use anchor_lang::prelude::*;
use derive_more::Deref;

pub mod account;
pub mod prover;

#[cfg(feature = "mainnet")]
pub const CHAIN_ID: u64 = 1399811149;
#[cfg(not(feature = "mainnet"))]
pub const CHAIN_ID: u64 = 1399811150;

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

impl IntoIterator for Bytes32 {
    type Item = u8;
    type IntoIter = std::array::IntoIter<u8, 32>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
