use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::v1::{self, MessageError, TransactionConfig};
use solana_sdk::message::{CompileError, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::{Signer, SignerError};
use solana_sdk::transaction::VersionedTransaction;

/// v1 budgets no compute units unless this is set, and `set_path_config`, which nests endpoint
/// and ULN302 CPIs, measured ~249k against devnet's ULN302.
pub const COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
/// The protocol maximum; a v1 transaction that leaves it unset may load nothing.
const LOADED_ACCOUNTS_DATA_SIZE_LIMIT: u32 = 64 * 1024 * 1024;
pub const MAX_SIZE: usize = v1::MAX_TRANSACTION_SIZE;
const MICRO_LAMPORTS_PER_LAMPORT: u128 = 1_000_000;
/// Any non-zero price: the priority fee's bytes do not depend on its value.
const SIZING_COMPUTE_UNIT_PRICE: u64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot compile the transaction: {source}")]
    CompileFailed {
        #[from]
        source: CompileError,
    },
    #[error("invalid v1 transaction message: {source}")]
    InvalidMessage {
        #[from]
        source: MessageError,
    },
    #[error("cannot sign the transaction: {source}")]
    SigningFailed {
        #[from]
        source: SignerError,
    },
}

/// A v1 transaction paid by the first signer. The compute-unit limit, the loaded-data limit and
/// the priority fee live in its config, since compute-budget instructions do nothing in v1.
pub fn signed(
    instructions: &[Instruction],
    signers: &[&Keypair],
    blockhash: Hash,
    compute_unit_price: u64,
) -> Result<VersionedTransaction, Error> {
    let payer = signers.first().expect("a transaction needs a fee payer");
    let message = message(&payer.pubkey(), instructions, blockhash, compute_unit_price)?;

    Ok(VersionedTransaction::try_new(message, signers)?)
}

/// Wire size of the transaction [`signed`] builds, with a priority fee so the size is the
/// largest any price yields.
pub fn size(payer: &Pubkey, instructions: &[Instruction]) -> Result<usize, Error> {
    let message = message(
        payer,
        instructions,
        Hash::default(),
        SIZING_COMPUTE_UNIT_PRICE,
    )?;
    let signatures: usize = message.header().num_required_signatures.into();
    let transaction = VersionedTransaction {
        signatures: vec![Signature::default(); signatures],
        message,
    };

    Ok(wincode::serialize(&transaction)
        .expect("a valid v1 transaction must serialize")
        .len())
}

/// Lamports for the whole compute-unit limit at `compute_unit_price` micro-lamports per unit,
/// rounded up.
pub fn priority_fee(compute_unit_price: u64) -> u64 {
    let price: u128 = compute_unit_price.into();
    let limit: u128 = COMPUTE_UNIT_LIMIT.into();

    (price * limit)
        .div_ceil(MICRO_LAMPORTS_PER_LAMPORT)
        .try_into()
        .unwrap_or(u64::MAX)
}

fn message(
    payer: &Pubkey,
    instructions: &[Instruction],
    blockhash: Hash,
    compute_unit_price: u64,
) -> Result<VersionedMessage, Error> {
    let message = v1::Message::try_compile_with_config(
        payer,
        instructions,
        blockhash,
        config(compute_unit_price),
    )?;
    message.validate()?;

    Ok(VersionedMessage::V1(message))
}

fn config(compute_unit_price: u64) -> TransactionConfig {
    let config = TransactionConfig::empty()
        .with_compute_unit_limit(COMPUTE_UNIT_LIMIT)
        .with_loaded_accounts_data_size_limit(LOADED_ACCOUNTS_DATA_SIZE_LIMIT);

    match compute_unit_price {
        0 => config,
        price => config.with_priority_fee(priority_fee(price)),
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::instruction::AccountMeta;

    use super::*;

    fn transfer(from: &Pubkey) -> Instruction {
        solana_system_interface::instruction::transfer(from, &Pubkey::new_unique(), 1)
    }

    fn signed_transfer(compute_unit_price: u64) -> VersionedTransaction {
        let payer = Keypair::new();

        signed(
            &[transfer(&payer.pubkey())],
            &[&payer],
            Hash::default(),
            compute_unit_price,
        )
        .unwrap()
    }

    fn config_of(compute_unit_price: u64) -> TransactionConfig {
        match signed_transfer(compute_unit_price).message {
            VersionedMessage::V1(message) => message.config,
            other => panic!("expected a v1 message, got {other:?}"),
        }
    }

    #[test]
    fn every_transaction_sets_the_compute_unit_and_loaded_data_limits() {
        [0, 1, u64::MAX].into_iter().for_each(|price| {
            let config = config_of(price);

            assert_eq!(config.compute_unit_limit, Some(1_400_000));
            assert_eq!(
                config.loaded_accounts_data_size_limit,
                Some(64 * 1024 * 1024)
            );
            assert_eq!(config.heap_size, None);
        });
    }

    #[test]
    fn a_zero_price_sets_no_priority_fee() {
        assert_eq!(config_of(0).priority_fee, None);
    }

    #[test]
    fn the_priority_fee_pays_the_whole_limit_rounded_up() {
        // 3 micro-lamports for 1.4M units is 4.2 lamports.
        assert_eq!(priority_fee(3), 5);
        assert_eq!(priority_fee(1_000_000), 1_400_000);
        assert_eq!(priority_fee(u64::MAX), u64::MAX);
        assert_eq!(config_of(3).priority_fee, Some(5));
    }

    #[test]
    fn no_compute_budget_instruction_is_added() {
        let message = signed_transfer(7).message;

        let programs: Vec<Pubkey> = message
            .instructions()
            .iter()
            .map(|instruction| message.static_account_keys()[instruction.program_id_index as usize])
            .collect();
        assert_eq!(programs, [solana_sdk_ids::system_program::id()]);
    }

    #[test]
    fn a_message_over_the_address_limit_is_refused() {
        let payer = Keypair::new();
        let accounts = (0..v1::MAX_ADDRESSES)
            .map(|_| AccountMeta::new_readonly(Pubkey::new_unique(), false))
            .collect();
        let instruction = Instruction {
            program_id: Pubkey::new_unique(),
            accounts,
            data: vec![],
        };

        let result = signed(&[instruction], &[&payer], Hash::default(), 0);

        assert!(matches!(
            result,
            Err(Error::InvalidMessage {
                source: MessageError::TooManyAddresses
            })
        ));
    }

    #[test]
    fn a_missing_signer_is_refused() {
        let payer = Keypair::new();

        let result = signed(
            &[transfer(&Pubkey::new_unique())],
            &[&payer],
            Hash::default(),
            0,
        );

        assert!(matches!(
            result,
            Err(Error::SigningFailed {
                source: SignerError::NotEnoughSigners
            })
        ));
    }

    #[test]
    fn size_is_the_signed_transaction_on_the_wire_at_any_price() {
        let payer = Keypair::new();
        let instructions = [transfer(&payer.pubkey())];
        let wire_size = |price| {
            let transaction = signed(&instructions, &[&payer], Hash::new_unique(), price).unwrap();

            wincode::serialize(&transaction).unwrap().len()
        };

        let size = size(&payer.pubkey(), &instructions).unwrap();

        assert_eq!(size, wire_size(5));
        assert_eq!(size, wire_size(0) + 8);
    }
}
