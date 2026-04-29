use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke;
use anchor_lang::InstructionData;
use portal::instructions::WithdrawArgs;
use portal::types::TokenTransferAccounts;

/// CPIs `portal.withdraw` to transfer the reward to `claimant`.
///
/// Trailing remaining accounts after the reward triples include `payer` as a
/// signer, which portal forwards into its `close_proof` CPI on the prover.
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
    let accounts = build_withdraw_metas(
        payer,
        claimant,
        vault,
        proof,
        proof_closer,
        prover,
        withdrawn_marker,
        token_program,
        token_2022_program,
        system_program,
        reward_transfers,
    );
    let infos = build_withdraw_infos(
        portal_program,
        payer,
        claimant,
        vault,
        proof,
        proof_closer,
        prover,
        withdrawn_marker,
        token_program,
        token_2022_program,
        system_program,
        reward_transfers,
    );

    let ix = Instruction {
        program_id: portal_program.key(),
        accounts,
        data: portal::instruction::Withdraw { args }.data(),
    };

    invoke(&ix, &infos).map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
fn build_withdraw_metas<'info>(
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
) -> Vec<AccountMeta> {
    let mut accounts = Vec::with_capacity(11 + 3 * reward_transfers.len());

    accounts.push(AccountMeta::new(payer.key(), true));
    accounts.push(AccountMeta::new(claimant.key(), false));
    accounts.push(AccountMeta::new(vault.key(), false));
    accounts.push(AccountMeta::new(proof.key(), false));
    accounts.push(AccountMeta::new_readonly(proof_closer.key(), false));
    accounts.push(AccountMeta::new_readonly(prover.key(), false));
    accounts.push(AccountMeta::new(withdrawn_marker.key(), false));
    accounts.push(AccountMeta::new_readonly(token_program.key(), false));
    accounts.push(AccountMeta::new_readonly(token_2022_program.key(), false));
    accounts.push(AccountMeta::new_readonly(system_program.key(), false));
    reward_transfers.iter().for_each(|transfer| {
        accounts.push(AccountMeta::new(transfer.from.key(), false));
        accounts.push(AccountMeta::new(transfer.to.key(), false));
        accounts.push(AccountMeta::new_readonly(transfer.mint.key(), false));
    });
    // Portal forwards withdraw's trailing remaining_accounts to
    // local-prover's close_proof CPI. close_proof expects `payer: Signer` at
    // the tail; pass our payer (same as the main `payer` above).
    accounts.push(AccountMeta::new(payer.key(), true));

    accounts
}

#[allow(clippy::too_many_arguments)]
fn build_withdraw_infos<'info>(
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
) -> Vec<AccountInfo<'info>> {
    let mut infos = Vec::with_capacity(12 + 3 * reward_transfers.len());

    infos.push(payer.to_account_info());
    infos.push(claimant.to_account_info());
    infos.push(vault.to_account_info());
    infos.push(proof.to_account_info());
    infos.push(proof_closer.to_account_info());
    infos.push(prover.to_account_info());
    infos.push(withdrawn_marker.to_account_info());
    infos.push(token_program.to_account_info());
    infos.push(token_2022_program.to_account_info());
    infos.push(system_program.to_account_info());

    reward_transfers.iter().for_each(|transfer| {
        infos.push(transfer.from.to_account_info());
        infos.push(transfer.to.to_account_info());
        infos.push(transfer.mint.to_account_info());
    });

    infos.push(payer.to_account_info());
    infos.push(portal_program.to_account_info());

    infos
}
