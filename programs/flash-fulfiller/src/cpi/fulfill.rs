use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::InstructionData;
use portal::instructions::FulfillArgs;
use portal::types::TokenTransferAccounts;

/// CPIs `portal.fulfill` with `solver` signed via `solver_seeds` (so that a PDA
/// like `flash_vault` can act as the solver).
#[allow(clippy::too_many_arguments)]
pub fn fulfill_intent<'info>(
    portal_program: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    solver: &AccountInfo<'info>,
    solver_seeds: &[&[u8]],
    executor: &AccountInfo<'info>,
    fulfill_marker: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
    token_2022_program: &AccountInfo<'info>,
    associated_token_program: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    route_transfers: &[TokenTransferAccounts<'info>],
    call_accounts: &[AccountInfo<'info>],
    args: FulfillArgs,
) -> Result<()> {
    // Pre-size both Vecs to avoid bump-allocator doubling waste. A plain
    // `chain(...).collect()` gives Vec::with_capacity(0) and then doubles,
    // retaining every prior buffer (Solana's bump allocator never frees). For
    // ~60 entries that's ~3 KB of dead heap per Vec — enough to push a
    // complex flash_fulfill path into OOM on the default 32 KB heap.
    const FIXED: usize = 8;
    let total_accounts = FIXED + route_transfers.len() * 3 + call_accounts.len();

    let mut accounts: Vec<AccountMeta> = Vec::with_capacity(total_accounts);
    accounts.push(AccountMeta::new(payer.key(), true));
    accounts.push(AccountMeta::new(solver.key(), true));
    accounts.push(AccountMeta::new(executor.key(), false));
    accounts.push(AccountMeta::new(fulfill_marker.key(), false));
    accounts.push(AccountMeta::new_readonly(token_program.key(), false));
    accounts.push(AccountMeta::new_readonly(token_2022_program.key(), false));
    accounts.push(AccountMeta::new_readonly(associated_token_program.key(), false));
    accounts.push(AccountMeta::new_readonly(system_program.key(), false));
    for transfer in route_transfers.iter() {
        accounts.push(AccountMeta::new(transfer.from.key(), false));
        accounts.push(AccountMeta::new(transfer.to.key(), false));
        accounts.push(AccountMeta::new_readonly(transfer.mint.key(), false));
    }
    for account in call_accounts.iter() {
        accounts.push(AccountMeta {
            pubkey: account.key(),
            is_signer: account.is_signer,
            is_writable: account.is_writable,
        });
    }

    let mut infos: Vec<AccountInfo<'info>> = Vec::with_capacity(total_accounts + 1);
    infos.push(payer.to_account_info());
    infos.push(solver.to_account_info());
    infos.push(executor.to_account_info());
    infos.push(fulfill_marker.to_account_info());
    infos.push(token_program.to_account_info());
    infos.push(token_2022_program.to_account_info());
    infos.push(associated_token_program.to_account_info());
    infos.push(system_program.to_account_info());
    for transfer in route_transfers.iter() {
        infos.push(transfer.from.to_account_info());
        infos.push(transfer.to.to_account_info());
        infos.push(transfer.mint.to_account_info());
    }
    for account in call_accounts.iter() {
        infos.push(account.to_account_info());
    }
    infos.push(portal_program.to_account_info());

    let ix = Instruction {
        program_id: portal_program.key(),
        accounts,
        data: portal::instruction::Fulfill { args }.data(),
    };

    invoke_signed(&ix, &infos, &[solver_seeds]).map_err(Into::into)
}
