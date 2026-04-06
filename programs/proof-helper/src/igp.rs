use anchor_lang::prelude::borsh::{BorshDeserialize, BorshSerialize};
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::system_program;

use crate::instructions::PayForGas as PayForGasAccounts;

#[cfg(feature = "mainnet")]
pub const IGP_PROGRAM_ID: Pubkey = pubkey!("BhNcatUDC2D5JTyeaqrdSukiVFsEHK7e3hVmKMztwefv");
#[cfg(not(feature = "mainnet"))]
pub const IGP_PROGRAM_ID: Pubkey = pubkey!("9SQVtTNsbipdMzumhzi6X8GwojiSMwBfqAhS7FgyTcqy");

#[cfg(feature = "mainnet")]
pub const MAILBOX_ID: Pubkey = pubkey!("E588QtVUvresuXq2KoNEwAmoifCzYGpRBdHByN9KQMbi");
#[cfg(not(feature = "mainnet"))]
pub const MAILBOX_ID: Pubkey = pubkey!("75HBBLae3ddeneJVrZeyrDfv6vb7SMC3aCpBucSXS5aR");

/// Hyperlane IGP instruction enum. Variant order is critical for Borsh
/// serialization — the `PayForGas` variant must remain at index 3.
#[derive(BorshSerialize, BorshDeserialize)]
#[allow(dead_code)]
pub enum IgpInstruction {
    Init,
    InitIgp(InitIgp),
    InitOverheadIgp(InitOverheadIgp),
    PayForGas(PayForGasData),
    QuoteGasPayment(QuoteGasPayment),
    TransferIgpOwnership(Option<Pubkey>),
    TransferOverheadIgpOwnership(Option<Pubkey>),
    SetIgpBeneficiary(Pubkey),
    SetDestinationGasOverheads(Vec<GasOverheadConfig>),
    SetGasOracleConfigs(Vec<GasOracleConfig>),
    Claim,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct InitIgp {
    pub salt: [u8; 32],
    pub owner: Option<Pubkey>,
    pub beneficiary: Pubkey,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct InitOverheadIgp {
    pub salt: [u8; 32],
    pub owner: Option<Pubkey>,
    pub inner: Pubkey,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct PayForGasData {
    pub message_id: [u8; 32],
    pub destination_domain: u32,
    pub gas_amount: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct QuoteGasPayment {
    pub destination_domain: u32,
    pub gas_amount: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct GasOverheadConfig {
    pub destination_domain: u32,
    pub gas_overhead: Option<u64>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct GasOracleConfig {
    pub domain: u32,
    pub gas_oracle: Option<GasOracle>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct GasOracle {
    pub token_exchange_rate: u128,
    pub gas_price: u128,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::prelude::borsh::BorshSerialize;

    /// PayForGas must remain at variant index 3 so that mock-igp (and the real
    /// Hyperlane IGP) can identify it from the first byte of serialized data.
    #[test]
    fn pay_for_gas_variant_is_index_3() {
        let ix = IgpInstruction::PayForGas(PayForGasData {
            message_id: [0u8; 32],
            destination_domain: 0,
            gas_amount: 0,
        });
        let serialized = ix.try_to_vec().unwrap();
        assert_eq!(serialized[0], 3, "PayForGas must be variant index 3");
    }

    /// Total serialized length must be exactly 45 bytes:
    /// variant(1) + message_id(32) + destination_domain(4) + gas_amount(8).
    /// This matches the minimum length check enforced by mock-igp.
    #[test]
    fn pay_for_gas_serialized_length_is_45() {
        let ix = IgpInstruction::PayForGas(PayForGasData {
            message_id: [0u8; 32],
            destination_domain: 0,
            gas_amount: 0,
        });
        let serialized = ix.try_to_vec().unwrap();
        assert_eq!(serialized.len(), 45);
    }

    /// The message_id occupies bytes [1..33] in the serialized instruction.
    #[test]
    fn pay_for_gas_message_id_bytes_1_to_33() {
        let message_id = [0xabu8; 32];
        let ix = IgpInstruction::PayForGas(PayForGasData {
            message_id,
            destination_domain: 0,
            gas_amount: 0,
        });
        let serialized = ix.try_to_vec().unwrap();
        assert_eq!(&serialized[1..33], &message_id);
    }

    /// The destination_domain occupies bytes [33..37] as a little-endian u32.
    /// This must match the mock-igp parsing: u32::from_le_bytes(data[33..37]).
    #[test]
    fn pay_for_gas_destination_domain_bytes_33_to_37_little_endian() {
        let domain: u32 = 0x12345678;
        let ix = IgpInstruction::PayForGas(PayForGasData {
            message_id: [0u8; 32],
            destination_domain: domain,
            gas_amount: 0,
        });
        let serialized = ix.try_to_vec().unwrap();
        let parsed = u32::from_le_bytes(serialized[33..37].try_into().unwrap());
        assert_eq!(parsed, domain);
    }

    /// The gas_amount occupies bytes [37..45] as a little-endian u64.
    /// This must match the mock-igp parsing: u64::from_le_bytes(data[37..45]).
    #[test]
    fn pay_for_gas_gas_amount_bytes_37_to_45_little_endian() {
        let gas: u64 = 0xdeadbeef_cafebabe;
        let ix = IgpInstruction::PayForGas(PayForGasData {
            message_id: [0u8; 32],
            destination_domain: 0,
            gas_amount: gas,
        });
        let serialized = ix.try_to_vec().unwrap();
        let parsed = u64::from_le_bytes(serialized[37..45].try_into().unwrap());
        assert_eq!(parsed, gas);
    }

    /// The serialized layout must match what mock-igp expects end-to-end.
    /// Builds a known instruction and verifies every field's position.
    #[test]
    fn pay_for_gas_full_layout_matches_mock_igp_expectation() {
        let message_id = [0x01u8; 32];
        let domain: u32 = 999;
        let gas: u64 = 100_000;

        let ix = IgpInstruction::PayForGas(PayForGasData {
            message_id,
            destination_domain: domain,
            gas_amount: gas,
        });
        let serialized = ix.try_to_vec().unwrap();

        // Mock-IGP checks: first byte == 3, len >= 45
        assert_eq!(serialized[0], 3);
        assert!(serialized.len() >= 45);

        // Field positions mirror mock-igp's parsing
        let parsed_domain = u32::from_le_bytes(serialized[33..37].try_into().unwrap());
        let parsed_gas = u64::from_le_bytes(serialized[37..45].try_into().unwrap());
        assert_eq!(parsed_domain, domain);
        assert_eq!(parsed_gas, gas);
    }

    /// Variant indices of the other IgpInstruction variants must remain stable.
    /// Changing them would break compatibility with on-chain IGP programs.
    #[test]
    fn igp_instruction_variant_indices_are_stable() {
        let init = IgpInstruction::Init;
        assert_eq!(init.try_to_vec().unwrap()[0], 0);

        let claim = IgpInstruction::Claim;
        // Claim is the 11th variant (0-indexed = 10)
        assert_eq!(claim.try_to_vec().unwrap()[0], 10);
    }

    /// IGP_PROGRAM_ID must be the expected devnet address.
    #[test]
    fn igp_program_id_is_deterministic() {
        goldie::assert_debug!(IGP_PROGRAM_ID);
    }

    /// MAILBOX_ID must be the expected devnet address.
    #[test]
    fn mailbox_id_is_deterministic() {
        goldie::assert_debug!(MAILBOX_ID);
    }
}

pub fn pay_for_gas(
    ctx: &Context<PayForGasAccounts>,
    message_id: [u8; 32],
    destination_domain: u32,
    gas_amount: u64,
) -> Result<()> {
    let igp_instruction = IgpInstruction::PayForGas(PayForGasData {
        message_id,
        destination_domain,
        gas_amount,
    });

    let mut accounts = vec![
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new(ctx.accounts.payer.key(), true),
        AccountMeta::new(ctx.accounts.igp_program_data.key(), false),
        AccountMeta::new_readonly(ctx.accounts.unique_gas_payment.key(), true),
        AccountMeta::new(ctx.accounts.gas_payment_pda.key(), false),
        AccountMeta::new(ctx.accounts.igp_account.key(), false),
    ];
    if let Some(overhead_igp) = &ctx.accounts.overhead_igp {
        accounts.push(AccountMeta::new_readonly(overhead_igp.key(), false));
    }

    let ix = Instruction {
        program_id: ctx.accounts.igp_program.key(),
        accounts,
        data: igp_instruction.try_to_vec()?,
    };

    let mut account_infos = vec![
        ctx.accounts.system_program.to_account_info(),
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.igp_program_data.to_account_info(),
        ctx.accounts.unique_gas_payment.to_account_info(),
        ctx.accounts.gas_payment_pda.to_account_info(),
        ctx.accounts.igp_account.to_account_info(),
    ];
    if let Some(overhead_igp) = &ctx.accounts.overhead_igp {
        account_infos.push(overhead_igp.to_account_info());
    }

    invoke(&ix, &account_infos).map_err(Into::into)
}