//! Test-only CPI relay, loaded under distinct IDs to model engine/venue/sub-program.
//! It is built explicitly for tests and excluded from production build scripts.

use anchor_lang::solana_program::account_info::AccountInfo;
use anchor_lang::solana_program::entrypoint;
use anchor_lang::solana_program::entrypoint::ProgramResult;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::program_error::ProgramError;
use anchor_lang::solana_program::pubkey::Pubkey;

entrypoint!(process_instruction);

/// Forwards data unchanged to accounts[0], passing all subsequent accounts.
/// Chaining distinct relay IDs adds one real SBF CPI frame per relay.
fn process_instruction(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let (target, remaining) = accounts
        .split_first()
        .ok_or(ProgramError::NotEnoughAccountKeys)?;
    invoke(
        &Instruction {
            program_id: *target.key,
            accounts: remaining
                .iter()
                .map(|account| AccountMeta {
                    pubkey: *account.key,
                    is_signer: account.is_signer,
                    is_writable: account.is_writable,
                })
                .collect(),
            data: data.to_vec(),
        },
        accounts,
    )
}
