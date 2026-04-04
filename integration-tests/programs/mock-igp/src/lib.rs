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
