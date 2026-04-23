use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use eco_svm_std::{Bytes32, CHAIN_ID};
use portal::types::{intent_hash, Reward};

use crate::instructions::FlashFulfillerError;
use crate::state::{FlashFulfillIntentAccount, FLASH_FULFILL_INTENT_SEED, MAX_ROUTE_INIT_SPACE};

/// Args for [`init_flash_fulfill_intent`]: commits an intent preimage and sizes the buffer.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitFlashFulfillIntentArgs {
    /// Keccak256 intent hash binding the PDA — must equal `keccak(CHAIN_ID, route_hash, reward.hash())`.
    pub intent_hash: Bytes32,
    /// Keccak256 of the route's Borsh encoding; later validated when the buffer fills.
    pub route_hash: Bytes32,
    /// Reward committed for this intent.
    pub reward: Reward,
    /// Total size (in bytes) of the Borsh-encoded Route the buffer will hold.
    pub route_total_size: u32,
}

/// Accounts for [`init_flash_fulfill_intent`].
#[derive(Accounts)]
pub struct InitFlashFulfillIntent<'info> {
    /// Pays rent and is recorded as the sole writer of this buffer.
    #[account(mut)]
    pub writer: Signer<'info>,
    /// CHECK: address + init handled in handler
    #[account(mut)]
    pub flash_fulfill_intent: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// Allocates a `FlashFulfillIntentAccount` buffer under `[seed, intent_hash, writer]`
/// committing `(route_hash, reward)` and pre-sizing `route_bytes` for later chunked writes.
pub fn init_flash_fulfill_intent(
    ctx: Context<InitFlashFulfillIntent>,
    args: InitFlashFulfillIntentArgs,
) -> Result<()> {
    let account = do_init(
        &ctx.accounts.writer.to_account_info(),
        &ctx.accounts.flash_fulfill_intent,
        &ctx.accounts.system_program,
        args,
    )?;

    serialize_buffer(&ctx.accounts.flash_fulfill_intent, &account)
}

/// Shared init logic: validates inputs, allocates the PDA, returns a fresh
/// in-memory `FlashFulfillIntentAccount`. Caller is responsible for any
/// subsequent in-memory mutations and the final serialize step so that
/// init+append can be fused atomically in one ix.
pub(crate) fn do_init<'info>(
    writer: &AccountInfo<'info>,
    buffer: &UncheckedAccount<'info>,
    system_program: &Program<'info, System>,
    args: InitFlashFulfillIntentArgs,
) -> Result<FlashFulfillIntentAccount> {
    let InitFlashFulfillIntentArgs {
        intent_hash: committed_intent_hash,
        route_hash,
        reward,
        route_total_size,
    } = args;

    let size = route_total_size as usize;
    require!(
        size > 0 && size <= MAX_ROUTE_INIT_SPACE,
        FlashFulfillerError::InvalidRouteTotalSize
    );

    require!(
        committed_intent_hash == intent_hash(CHAIN_ID, &route_hash, &reward.hash()),
        FlashFulfillerError::InvalidIntentHash
    );

    let writer_key = writer.key();
    let (expected_pda, bump) = FlashFulfillIntentAccount::pda(&committed_intent_hash, &writer_key);
    require!(
        buffer.key() == expected_pda,
        FlashFulfillerError::InvalidFlashFulfillIntentAccount
    );
    // Matches `AccountExt::init`'s pre-allocation guard so a second init at the
    // same PDA surfaces as `ConstraintZero` rather than a system-program error.
    require!(
        buffer.data_is_empty() && *buffer.owner != crate::ID,
        anchor_lang::error::ErrorCode::ConstraintZero
    );

    let data_len = 8 + FlashFulfillIntentAccount::account_space(route_total_size);
    let rent = Rent::get()?.minimum_balance(data_len);
    let bump_arr = [bump];
    let signer_seeds: &[&[u8]] = &[
        FLASH_FULFILL_INTENT_SEED,
        committed_intent_hash.as_ref(),
        writer_key.as_ref(),
        &bump_arr,
    ];
    let account_infos = [
        writer.to_account_info(),
        buffer.to_account_info(),
        system_program.to_account_info(),
    ];

    // Handle the grief where an attacker has pre-funded the PDA with a small
    // lamport transfer. In that case `create_account` would fail, so fall
    // back to top-up + allocate + assign. Mirrors `AccountExt::init`.
    match buffer.lamports() {
        0 => {
            invoke_signed(
                &system_instruction::create_account(
                    &writer_key,
                    &expected_pda,
                    rent,
                    data_len as u64,
                    &crate::ID,
                ),
                &account_infos,
                &[signer_seeds],
            )?;
        }
        current_balance => {
            if let Some(topup) = rent.checked_sub(current_balance).filter(|a| *a > 0) {
                invoke_signed(
                    &system_instruction::transfer(&writer_key, &expected_pda, topup),
                    &account_infos,
                    &[signer_seeds],
                )?;
            }
            invoke_signed(
                &system_instruction::allocate(&expected_pda, data_len as u64),
                &[buffer.to_account_info(), system_program.to_account_info()],
                &[signer_seeds],
            )?;
            invoke_signed(
                &system_instruction::assign(&expected_pda, &crate::ID),
                &[buffer.to_account_info(), system_program.to_account_info()],
                &[signer_seeds],
            )?;
        }
    }

    Ok(FlashFulfillIntentAccount {
        writer: writer_key,
        reward,
        route_hash,
        route_total_size,
        route_bytes_written: 0,
        created_at: Clock::get()?.unix_timestamp,
        finalized: false,
        route_bytes: vec![0u8; size],
    })
}

/// Writes the in-memory buffer state into the underlying account data,
/// including the 8-byte Anchor discriminator.
pub(crate) fn serialize_buffer(
    buffer_ai: &UncheckedAccount,
    account: &FlashFulfillIntentAccount,
) -> Result<()> {
    let mut data = buffer_ai.try_borrow_mut_data()?;
    account.try_serialize(&mut &mut data[..])
}
