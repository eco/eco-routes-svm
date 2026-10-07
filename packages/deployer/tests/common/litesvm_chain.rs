use anchor_lang::{AnchorDeserialize, AnchorSerialize, Discriminator};
use deployer::{chain, Chain};
use layerzero_prover::instructions::PathConfig;
use layerzero_prover::layerzero::{uln_receive_config_pda, uln_send_config_pda, ULN_ID};
use solana_sdk::account::Account;
use solana_sdk::clock::Clock;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::common::Context;

pub struct LitesvmChain<'a>(pub &'a mut Context);

impl Chain for LitesvmChain<'_> {
    fn account(&self, address: &Pubkey) -> Result<Option<Account>, chain::Error> {
        Ok(self.0.get_account(address))
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<Signature, chain::Error> {
        let payer = signers.first().expect("a transaction needs a fee payer");
        let transaction = Transaction::new(
            signers,
            Message::new(instructions, Some(&payer.pubkey())),
            self.0.latest_blockhash(),
        );

        let signature = self
            .0
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
        Ok(self.0.get_sysvar::<Clock>().slot)
    }

    fn genesis_hash(&self) -> Result<Hash, chain::Error> {
        Ok(self.0.genesis_hash)
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
            self.0.set_account(address, config).unwrap();
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
