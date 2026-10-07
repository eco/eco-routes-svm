use solana_compute_budget_interface::ComputeBudgetInstruction;
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

/// Prepends a compute-unit price to every transaction; `0` leaves transactions untouched.
pub struct Priced<C> {
    chain: C,
    micro_lamports: u64,
}

impl<C> Priced<C> {
    pub fn new(chain: C, micro_lamports: u64) -> Self {
        Self {
            chain,
            micro_lamports,
        }
    }
}

impl<C: Chain> Chain for Priced<C> {
    fn account(&self, address: &Pubkey) -> Result<Option<Account>, Error> {
        self.chain.account(address)
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, Error> {
        match self.micro_lamports {
            0 => self.chain.send(instructions, signers),
            micro_lamports => {
                let priced: Vec<Instruction> = [ComputeBudgetInstruction::set_compute_unit_price(
                    micro_lamports,
                )]
                .into_iter()
                .chain(instructions.iter().cloned())
                .collect();

                self.chain.send(&priced, signers)
            }
        }
    }

    fn slot(&self) -> Result<u64, Error> {
        self.chain.slot()
    }

    fn genesis_hash(&self) -> Result<Hash, Error> {
        self.chain.genesis_hash()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingChain;

    fn transfer() -> Instruction {
        solana_system_interface::instruction::transfer(
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            1,
        )
    }

    #[test]
    fn a_price_is_prepended_to_every_transaction() {
        let mut chain = Priced::new(RecordingChain::default(), 7);

        chain.send(&[transfer()], &[]).unwrap();
        chain.send(&[transfer()], &[]).unwrap();

        let sent = &chain.chain.sent;
        let price = ComputeBudgetInstruction::set_compute_unit_price(7);
        assert_eq!(sent.len(), 4);
        assert_eq!([&sent[0], &sent[2]], [&price, &price]);
    }

    #[test]
    fn a_zero_price_leaves_transactions_untouched() {
        let mut chain = Priced::new(RecordingChain::default(), 0);

        chain.send(&[transfer()], &[]).unwrap();

        assert_eq!(chain.chain.sent.len(), 1);
        assert_eq!(
            chain.chain.sent[0].program_id,
            solana_sdk_ids::system_program::id()
        );
    }
}
