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

/// Serializable version of Solana's `AccountMeta` for cross-chain communication.
///
/// Since Solana's native `AccountMeta` type doesn't implement serialization traits
/// required for cross-chain messaging, this struct provides a serializable equivalent
/// that can be included in `CallDataWithAccounts` and transmitted across chains.
///
/// This allows account metadata to be reconstructed on the destination chain
/// during intent fulfillment, enabling proper validation and execution.
#[derive(AnchorDeserialize, AnchorSerialize, Debug)]
pub struct SerializableAccountMeta {
    /// The account's public key
    pub pubkey: Pubkey,
    /// Whether this account must sign the transaction
    pub is_signer: bool,
    /// Whether this account's data may be modified
    pub is_writable: bool,
}

impl From<AccountInfo<'_>> for SerializableAccountMeta {
    fn from(account_info: AccountInfo<'_>) -> Self {
        Self {
            pubkey: account_info.key(),
            is_signer: account_info.is_signer,
            is_writable: account_info.is_writable,
        }
    }
}

impl From<AccountMeta> for SerializableAccountMeta {
    fn from(account_meta: AccountMeta) -> Self {
        Self {
            pubkey: account_meta.pubkey,
            is_signer: account_meta.is_signer,
            is_writable: account_meta.is_writable,
        }
    }
}
