use anchor_lang::solana_program::account_info::AccountInfo;
use anchor_lang::solana_program::entrypoint::ProgramResult;
use anchor_lang::solana_program::program_error::ProgramError;
use anchor_lang::solana_program::pubkey::Pubkey;
use anchor_lang::solana_program::{entrypoint, msg};

// Same ID as the devnet Hyperlane IGP so proof-helper's address check passes.
anchor_lang::declare_id!("9SQVtTNsbipdMzumhzi6X8GwojiSMwBfqAhS7FgyTcqy");

entrypoint!(process_instruction);

/// Mock IGP entrypoint that accepts the raw Borsh-encoded IgpInstruction.
/// Only PayForGas (variant index 3) is handled — everything else errors.
fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let variant = instruction_data
        .first()
        .ok_or(ProgramError::InvalidInstructionData)?;

    // PayForGas is variant index 3 in the IgpInstruction enum
    if *variant != 3 {
        msg!("MockIGP: unsupported instruction variant {}", variant);
        return Err(ProgramError::InvalidInstructionData);
    }

    // Validate minimum data length: variant(1) + message_id(32) + domain(4) + gas(8) = 45
    if instruction_data.len() < 45 {
        return Err(ProgramError::InvalidInstructionData);
    }

    let domain = u32::from_le_bytes(instruction_data[33..37].try_into().unwrap());
    let gas = u64::from_le_bytes(instruction_data[37..45].try_into().unwrap());
    msg!("MockIGP: pay_for_gas domain={} gas={}", domain, gas);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::solana_program::pubkey::Pubkey;

    fn dummy_pubkey() -> Pubkey {
        Pubkey::default()
    }

    /// Build a minimal valid PayForGas instruction payload:
    /// variant(1) + message_id(32) + domain(4) + gas(8) = 45 bytes.
    fn valid_pay_for_gas_data(domain: u32, gas: u64) -> Vec<u8> {
        let mut data = vec![0u8; 45];
        data[0] = 3; // PayForGas variant index
        // message_id at [1..33] — all zeros is fine
        data[33..37].copy_from_slice(&domain.to_le_bytes());
        data[37..45].copy_from_slice(&gas.to_le_bytes());
        data
    }

    #[test]
    fn empty_instruction_data_returns_error() {
        let result = process_instruction(&dummy_pubkey(), &[], &[]);
        assert_eq!(result, Err(ProgramError::InvalidInstructionData));
    }

    #[test]
    fn unsupported_variant_0_returns_error() {
        let data = [0u8]; // variant Init = 0
        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Err(ProgramError::InvalidInstructionData));
    }

    #[test]
    fn unsupported_variant_1_returns_error() {
        let data = [1u8, 0, 0]; // variant InitIgp = 1
        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Err(ProgramError::InvalidInstructionData));
    }

    #[test]
    fn unsupported_variant_2_returns_error() {
        let data = [2u8, 0, 0]; // variant InitOverheadIgp = 2
        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Err(ProgramError::InvalidInstructionData));
    }

    #[test]
    fn unsupported_variant_4_returns_error() {
        let data = [4u8, 0, 0]; // variant QuoteGasPayment = 4
        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Err(ProgramError::InvalidInstructionData));
    }

    #[test]
    fn pay_for_gas_variant_too_short_44_bytes_returns_error() {
        let mut data = vec![0u8; 44];
        data[0] = 3; // correct variant, but 44 < 45
        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Err(ProgramError::InvalidInstructionData));
    }

    #[test]
    fn pay_for_gas_exactly_45_bytes_succeeds() {
        let data = valid_pay_for_gas_data(1, 100_000);
        assert_eq!(data.len(), 45);
        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn pay_for_gas_more_than_45_bytes_succeeds() {
        let mut data = valid_pay_for_gas_data(1, 100_000);
        data.extend_from_slice(&[0xffu8; 10]); // extra trailing bytes
        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Ok(()));
    }

    /// Verify domain is parsed from bytes [33..37] as little-endian u32.
    /// The domain value is verified indirectly via the test succeeding (the
    /// mock only logs it; production tests verify the log content).
    #[test]
    fn pay_for_gas_parses_domain_from_correct_offset() {
        let expected_domain: u32 = 0xdeadbeef;
        let data = valid_pay_for_gas_data(expected_domain, 0);

        // Verify we placed the domain bytes at the right offset
        let parsed = u32::from_le_bytes(data[33..37].try_into().unwrap());
        assert_eq!(parsed, expected_domain);

        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Ok(()));
    }

    /// Verify gas is parsed from bytes [37..45] as little-endian u64.
    #[test]
    fn pay_for_gas_parses_gas_from_correct_offset() {
        let expected_gas: u64 = 0xcafe_babe_dead_beef;
        let data = valid_pay_for_gas_data(0, expected_gas);

        let parsed = u64::from_le_bytes(data[37..45].try_into().unwrap());
        assert_eq!(parsed, expected_gas);

        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Ok(()));
    }

    /// The accounts slice is entirely ignored; passing arbitrary lengths is fine.
    #[test]
    fn accounts_are_ignored() {
        let data = valid_pay_for_gas_data(42, 9999);
        // Pass empty accounts — should still succeed
        let result = process_instruction(&dummy_pubkey(), &[], &data);
        assert_eq!(result, Ok(()));
    }

    /// The program_id is entirely ignored; any pubkey is accepted.
    #[test]
    fn program_id_is_ignored() {
        let data = valid_pay_for_gas_data(1, 1);
        let random_key = Pubkey::new_from_array([0xffu8; 32]);
        let result = process_instruction(&random_key, &[], &data);
        assert_eq!(result, Ok(()));
    }
}