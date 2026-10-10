use anchor_lang::{AnchorDeserialize, AnchorSerialize, Discriminator};
use deployer::{chain, transaction, Chain};
use layerzero_prover::instructions::PathConfig;
use layerzero_prover::layerzero::{uln_receive_config_pda, uln_send_config_pda, ULN_ID};
use solana_sdk::account::Account;
use solana_sdk::clock::Clock;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};

use crate::common::Context;

pub struct LitesvmChain<'a> {
    context: &'a mut Context,
    /// Micro-lamports per compute unit, paid as each transaction's priority fee.
    compute_unit_price: u64,
}

impl<'a> LitesvmChain<'a> {
    pub fn new(context: &'a mut Context) -> Self {
        Self {
            context,
            compute_unit_price: 0,
        }
    }

    pub fn with_compute_unit_price(self, compute_unit_price: u64) -> Self {
        Self {
            compute_unit_price,
            ..self
        }
    }
}

impl Chain for LitesvmChain<'_> {
    fn account(&self, address: &Pubkey) -> Result<Option<Account>, chain::Error> {
        Ok(self.context.get_account(address))
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, chain::Error> {
        let transaction = transaction::signed(
            instructions,
            signers,
            self.context.latest_blockhash(),
            self.compute_unit_price,
        )
        .map_err(|error| chain::Error::SendFailed {
            reason: error.to_string(),
        })?;

        let signature = self
            .context
            .send_transaction(transaction)
            .map(|metadata| metadata.signature)
            .map_err(|failure| chain::Error::SendFailed {
                reason: format!("{failure:?}"),
            })?;
        instructions
            .iter()
            .filter(|instruction| is_set_path_config(instruction))
            .for_each(|instruction| self.stage_uln_configs(instruction));

        Ok(signature)
    }

    fn slot(&self) -> Result<u64, chain::Error> {
        Ok(self.context.get_sysvar::<Clock>().slot)
    }

    fn genesis_hash(&self) -> Result<Hash, chain::Error> {
        Ok(self.context.genesis_hash)
    }
}

impl LitesvmChain<'_> {
    /// The mock endpoint never creates the per-OApp ULN accounts that the real ULN302's
    /// `init_config`/`set_config` write; stage what ULN302 would store for this instruction.
    fn stage_uln_configs(&mut self, set_path_config: &Instruction) {
        let layerzero_prover::instruction::SetPathConfig { eid, config } =
            AnchorDeserialize::deserialize(&mut &set_path_config.data[8..]).unwrap();
        let store = set_path_config.accounts[3].pubkey;
        let PathConfig {
            send_uln,
            receive_uln,
            executor,
        } = config;
        let send = uln_account_data("SendConfig", &(255u8, send_uln, executor));
        let receive = uln_account_data("ReceiveConfig", &(255u8, receive_uln));

        [
            (uln_send_config_pda(eid, &store).0, send),
            (uln_receive_config_pda(eid, &store).0, receive),
        ]
        .into_iter()
        .for_each(|(address, data)| {
            let config = Account {
                lamports: 1_000_000,
                data,
                owner: ULN_ID,
                ..Account::default()
            };
            self.context.set_account(address, config).unwrap();
        });
    }
}

/// An Anchor account as ULN302 stores it: discriminator, then the Borsh fields.
pub fn uln_account_data(name: &str, fields: &impl AnchorSerialize) -> Vec<u8> {
    let mut data =
        solana_sha256_hasher::hash(format!("account:{name}").as_bytes()).to_bytes()[..8].to_vec();
    fields.serialize(&mut data).unwrap();

    data
}

fn is_set_path_config(instruction: &Instruction) -> bool {
    instruction.program_id == layerzero_prover::ID
        && instruction
            .data
            .starts_with(layerzero_prover::instruction::SetPathConfig::DISCRIMINATOR)
}
