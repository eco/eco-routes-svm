use std::iter;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke;
use anchor_lang::InstructionData;
use portal::instructions::WithdrawArgs;
use portal::types::TokenTransferAccounts;

#[allow(clippy::too_many_arguments)]
pub fn withdraw_intent<'info>(
    portal_program: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    claimant: &AccountInfo<'info>,
    vault: &AccountInfo<'info>,
    proof: &AccountInfo<'info>,
    proof_closer: &AccountInfo<'info>,
    prover: &AccountInfo<'info>,
    withdrawn_marker: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
    token_2022_program: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    reward_transfers: &[TokenTransferAccounts<'info>],
    args: WithdrawArgs,
) -> Result<()> {
    let fixed_metas = [
        AccountMeta::new(payer.key(), true),
        AccountMeta::new(claimant.key(), false),
        AccountMeta::new(vault.key(), false),
        AccountMeta::new(proof.key(), false),
        AccountMeta::new_readonly(proof_closer.key(), false),
        AccountMeta::new_readonly(prover.key(), false),
        AccountMeta::new(withdrawn_marker.key(), false),
        AccountMeta::new_readonly(token_program.key(), false),
        AccountMeta::new_readonly(token_2022_program.key(), false),
        AccountMeta::new_readonly(system_program.key(), false),
    ];
    let reward_metas = reward_transfers.iter().flat_map(|transfer| {
        [
            AccountMeta::new(transfer.from.key(), false),
            AccountMeta::new(transfer.to.key(), false),
            AccountMeta::new_readonly(transfer.mint.key(), false),
        ]
    });
    let accounts: Vec<AccountMeta> = fixed_metas
        .into_iter()
        .chain(reward_metas)
        .chain(iter::once(AccountMeta::new(payer.key(), true)))
        .collect();

    let fixed_infos = [
        payer.to_account_info(),
        claimant.to_account_info(),
        vault.to_account_info(),
        proof.to_account_info(),
        proof_closer.to_account_info(),
        prover.to_account_info(),
        withdrawn_marker.to_account_info(),
        token_program.to_account_info(),
        token_2022_program.to_account_info(),
        system_program.to_account_info(),
    ];
    let reward_infos = reward_transfers.iter().flat_map(|transfer| {
        [
            transfer.from.to_account_info(),
            transfer.to.to_account_info(),
            transfer.mint.to_account_info(),
        ]
    });
    let infos: Vec<AccountInfo<'info>> = fixed_infos
        .into_iter()
        .chain(reward_infos)
        .chain(iter::once(payer.to_account_info()))
        .chain(iter::once(portal_program.to_account_info()))
        .collect();

    let ix = Instruction {
        program_id: portal_program.key(),
        accounts,
        data: portal::instruction::Withdraw { args }.data(),
    };

    invoke(&ix, &infos).map_err(Into::into)
}
