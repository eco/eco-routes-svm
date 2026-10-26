use solana_sdk::account::Account;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to fetch account {address}: {reason}")]
    AccountFetchFailed { address: Pubkey, reason: String },
    #[error("failed to send transaction: {reason}")]
    SendFailed { reason: String },
    #[error("failed to fetch slot: {reason}")]
    SlotFetchFailed { reason: String },
    #[error("failed to fetch genesis hash: {reason}")]
    GenesisHashFetchFailed { reason: String },
}

pub trait Chain {
    fn account(&self, address: &Pubkey) -> Result<Option<Account>, Error>;

    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, Error>;

    fn slot(&self) -> Result<u64, Error>;

    /// Identifies the cluster: devnet and mainnet share program IDs, so the address alone does
    /// not.
    fn genesis_hash(&self) -> Result<Hash, Error>;
}
