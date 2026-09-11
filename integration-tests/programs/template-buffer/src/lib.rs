//! Non-production transport fixture. Never included in release/deploy lists.
//! Packet-sized writes followed by a generic CPI; no escrow, signing PDA or adapter.
#![allow(unexpected_cfgs)]
use anchor_lang::solana_program::account_info::AccountInfo;
use anchor_lang::solana_program::entrypoint;
use anchor_lang::solana_program::entrypoint::ProgramResult;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::program_error::ProgramError;
use anchor_lang::solana_program::pubkey::Pubkey;

#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process);

pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let (buffer, rest) = accounts
        .split_first()
        .ok_or(ProgramError::NotEnoughAccountKeys)?;
    if buffer.owner != program_id {
        return Err(ProgramError::IncorrectProgramId);
    }
    // Explicit log-pressure mode for durable-event tests, without hashing a
    // dummy route or spending most of the transaction's compute budget on it.
    if data == [255] {
        for _ in 0..3 {
            anchor_lang::solana_program::log::sol_log_data(&[&buffer.try_borrow_data()?]);
        }
        return Ok(());
    }
    if !data.is_empty() {
        if !buffer.is_signer || data.len() < 4 {
            return Err(ProgramError::InvalidArgument);
        }
        let offset = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
        let payload = &data[4..];
        buffer
            .try_borrow_mut_data()?
            .get_mut(offset..offset + payload.len())
            .ok_or(ProgramError::AccountDataTooSmall)?
            .copy_from_slice(payload);
        return Ok(());
    }
    let (target, forwarded) = rest
        .split_first()
        .ok_or(ProgramError::NotEnoughAccountKeys)?;
    let ix = Instruction {
        program_id: *target.key,
        accounts: forwarded
            .iter()
            .map(|a| AccountMeta {
                pubkey: *a.key,
                is_signer: a.is_signer,
                is_writable: a.is_writable,
            })
            .collect(),
        data: buffer.try_borrow_data()?.to_vec(),
    };
    invoke(&ix, rest)
}
