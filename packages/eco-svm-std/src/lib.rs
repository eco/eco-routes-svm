use anchor_lang::prelude::*;

pub mod account;
pub mod prover;

#[cfg(feature = "mainnet")]
pub const CHAIN_ID: u64 = 1399811149;
#[cfg(not(feature = "mainnet"))]
pub const CHAIN_ID: u64 = 1399811150;

const EVENT_AUTHORITY_SEED: &[u8] = b"__event_authority";

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bytes32([u8; 32]);

impl std::ops::Deref for Bytes32 {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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

/// Claimant sentinel written by the destination portal's `cancel` instruction.
///
/// `keccak256("eco.portal.intent.cancelled")`, byte-identical to EVM
/// `Inbox.CANCELLED`, so a cancellation travels as an ordinary
/// `(intent_hash, claimant)` pair on every prover. Nobody holds a key for it
/// (it is a hash output), and its upper 12 bytes are non-zero, so EVM provers
/// never read it as an address. On the source, a `Proof` whose claimant is
/// `CANCELLED` is a proven cancellation: `refund` accepts it before
/// `reward.deadline`, and `withdraw` rejects it.
pub const CANCELLED: Bytes32 = Bytes32([
    0xa8, 0xaa, 0x89, 0x81, 0x26, 0x67, 0x9f, 0x5f, 0x17, 0x9c, 0xb3, 0xa4, 0xe6, 0x85, 0x05, 0x6a,
    0xec, 0x77, 0x68, 0x6a, 0x83, 0xe2, 0xa6, 0xbd, 0xf3, 0x7c, 0x6f, 0x71, 0xdd, 0x2f, 0xdb, 0x5f,
]);

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

pub fn event_authority_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[EVENT_AUTHORITY_SEED], program_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_authority_pda_deterministic() {
        let program_id = Pubkey::new_from_array([123u8; 32]);

        goldie::assert_debug!(event_authority_pda(&program_id));
    }

    #[test]
    fn cancelled_is_keccak_of_its_domain_tag() {
        use tiny_keccak::{Hasher, Keccak};

        let mut hasher = Keccak::v256();
        let mut hash = [0u8; 32];
        hasher.update(b"eco.portal.intent.cancelled");
        hasher.finalize(&mut hash);

        assert_eq!(<[u8; 32]>::from(CANCELLED), hash);
    }

    /// The same 32 bytes are `Inbox.CANCELLED` on EVM; a drift here makes one
    /// VM's cancellation look like a real claimant to the other.
    #[test]
    fn cancelled_matches_the_evm_constant() {
        let hex: String = CANCELLED.iter().map(|byte| format!("{byte:02x}")).collect();

        assert_eq!(
            hex,
            "a8aa898126679f5f179cb3a4e685056aec77686a83e2a6bdf37c6f71dd2fdb5f"
        );
    }

    /// Upper 12 bytes non-zero: EVM provers can never mistake it for an address.
    #[test]
    fn cancelled_is_not_an_evm_address() {
        assert!(CANCELLED[..12].iter().any(|byte| *byte != 0));
    }

    #[test]
    fn cancelled_deterministic() {
        goldie::assert_debug!(CANCELLED);
    }
}
