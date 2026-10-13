use anchor_lang::{system_program, InstructionData, ToAccountMetas};
use cctp_prover::instructions::InitArgs;
use cctp_prover::state::{Peer, CONFIG_SEED};
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;

use crate::plan::CCTP_PROVER;
use crate::{config, transaction};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Transaction(#[from] transaction::Error),
    #[error("{CCTP_PROVER} init needs {size} bytes, over the {limit}-byte transaction limit; use fewer peers")]
    TransactionTooLarge { size: usize, limit: usize },
}

/// `init` with every peer, at the release address rather than the compiled-in ID; the deployer
/// is payer and upgrade authority.
pub fn init_instruction(program: &Pubkey, deployer: &Pubkey, peers: Vec<Peer>) -> Instruction {
    let accounts = cctp_prover::accounts::Init {
        payer: *deployer,
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        config: config::config_address(CONFIG_SEED, program),
        system_program: system_program::ID,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: cctp_prover::instruction::Init {
            args: InitArgs { peers },
        }
        .data(),
    }
}

/// Rejects a peer set whose `init` would not fit one transaction as `apply` sends it (a v1
/// transaction, deployer as sole signer). Past about 19 peers it no longer fits a v0 one.
pub fn check_transaction_size(program: &Pubkey, peers: &[Peer]) -> Result<(), Error> {
    let deployer = Pubkey::new_from_array([1; 32]);
    let size = transaction::size(
        &deployer,
        &[init_instruction(program, &deployer, peers.to_vec())],
    )?;

    match size <= transaction::MAX_SIZE {
        true => Ok(()),
        false => Err(Error::TransactionTooLarge {
            size,
            limit: transaction::MAX_SIZE,
        }),
    }
}

#[cfg(test)]
mod tests {
    use cctp_prover::state::MAX_PEERS;
    use solana_sdk::instruction::AccountMeta;

    use super::*;

    const LEGACY_PACKET_SIZE: usize = 1232;

    fn peers(count: usize) -> Vec<Peer> {
        (0..count as u32)
            .map(|index| Peer {
                domain: 100 + index,
                chain_id: 900_000 + u64::from(index),
                sender: [index as u8 + 1; 32].into(),
            })
            .collect()
    }

    #[test]
    fn init_accounts_follow_the_program_order() {
        let program = Pubkey::new_unique();
        let deployer = Pubkey::new_unique();

        let instruction = init_instruction(&program, &deployer, peers(1));

        assert_eq!(instruction.program_id, program);
        assert_eq!(
            instruction.accounts,
            [
                AccountMeta::new(deployer, true),
                AccountMeta::new_readonly(deployer, true),
                AccountMeta::new_readonly(program, false),
                AccountMeta::new_readonly(config::program_data_address(&program), false),
                AccountMeta::new(
                    Pubkey::find_program_address(&[CONFIG_SEED], &program).0,
                    false
                ),
                AccountMeta::new_readonly(system_program::ID, false),
            ]
        );
    }

    #[test]
    fn the_maximum_number_of_peers_fits_one_v1_transaction_but_not_a_v0_packet() {
        let program = Pubkey::new_unique();
        let deployer = Pubkey::new_from_array([1; 32]);
        let size = transaction::size(
            &deployer,
            &[init_instruction(&program, &deployer, peers(MAX_PEERS))],
        )
        .unwrap();

        assert!(check_transaction_size(&program, &peers(MAX_PEERS)).is_ok());
        assert!(size > LEGACY_PACKET_SIZE, "{size}");
    }
}
