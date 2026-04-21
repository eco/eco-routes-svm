use std::iter;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::InstructionData;
use portal::instructions::FulfillArgs;
use portal::types::TokenTransferAccounts;

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
    let fixed_metas = [
        AccountMeta::new(payer.key(), true),
        AccountMeta::new(solver.key(), true),
        AccountMeta::new(executor.key(), false),
        AccountMeta::new(fulfill_marker.key(), false),
        AccountMeta::new_readonly(token_program.key(), false),
        AccountMeta::new_readonly(token_2022_program.key(), false),
        AccountMeta::new_readonly(associated_token_program.key(), false),
        AccountMeta::new_readonly(system_program.key(), false),
    ];
    let route_metas = route_transfers.iter().flat_map(|transfer| {
        [
            AccountMeta::new(transfer.from.key(), false),
            AccountMeta::new(transfer.to.key(), false),
            AccountMeta::new_readonly(transfer.mint.key(), false),
        ]
    });
    let call_metas = call_accounts.iter().map(|account| AccountMeta {
        pubkey: account.key(),
        is_signer: account.is_signer,
        is_writable: account.is_writable,
    });
    let accounts: Vec<AccountMeta> = fixed_metas
        .into_iter()
        .chain(route_metas)
        .chain(call_metas)
        .collect();

    let fixed_infos = [
        payer.to_account_info(),
        solver.to_account_info(),
        executor.to_account_info(),
        fulfill_marker.to_account_info(),
        token_program.to_account_info(),
        token_2022_program.to_account_info(),
        associated_token_program.to_account_info(),
        system_program.to_account_info(),
    ];
    let route_infos = route_transfers.iter().flat_map(|transfer| {
        [
            transfer.from.to_account_info(),
            transfer.to.to_account_info(),
            transfer.mint.to_account_info(),
        ]
    });
    let call_infos = call_accounts.iter().map(ToAccountInfo::to_account_info);
    let infos: Vec<AccountInfo<'info>> = fixed_infos
        .into_iter()
        .chain(route_infos)
        .chain(call_infos)
        .chain(iter::once(portal_program.to_account_info()))
        .collect();

    let ix = Instruction {
        program_id: portal_program.key(),
        accounts,
        data: portal::instruction::Fulfill { args }.data(),
    };

    invoke_signed(&ix, &infos, &[solver_seeds]).map_err(Into::into)
}
