use anchor_lang::prelude::*;
use anchor_spl::{token, token_2022};
use eco_svm_std::{Bytes32, CHAIN_ID};
use portal::types::{
    self, Reward, TokenTransferAccounts, VecTokenTransferAccounts,
    VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
};

use crate::instructions::FlashFulfillerError;
use crate::paired_fulfill::require_paired_fulfill;
use crate::prove_withdraw::ProveWithdraw;
use crate::state::prove_authority_pda;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ProveAndWithdrawArgs {
    pub route_hash: Bytes32,
    pub reward: Reward,
}

#[derive(Accounts)]
pub struct ProveAndWithdraw<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut)]
    pub solver: Signer<'info>,
    /// CHECK: pinned to the runtime's top-level instructions sysvar.
    #[account(address = solana_instructions_sysvar::ID)]
    pub instructions: UncheckedAccount<'info>,
    /// CHECK: initialized and validated by the prover, then validated by portal.
    #[account(mut)]
    pub proof: UncheckedAccount<'info>,
    /// CHECK: validated by portal.withdraw against vault_pda(intent_hash).
    #[account(mut)]
    pub intent_vault: UncheckedAccount<'info>,
    /// CHECK: validated and initialized by portal.withdraw; prevents replay.
    #[account(mut)]
    pub withdrawn_marker: UncheckedAccount<'info>,
    /// CHECK: pinned so the withdraw leg must execute the trusted portal.
    #[account(executable, address = portal::ID @ FlashFulfillerError::InvalidPortalProgram)]
    pub portal_program: UncheckedAccount<'info>,
    /// CHECK: executable-only, as in flash_fulfill (local-prover depends on us).
    /// The program in this slot receives `payer` as a writable signer, so a payer
    /// that is not the solver must pin it to the release's local prover. The
    /// signed prove authority is scoped to this program, and portal validates
    /// this key against reward.prover and the shared proof PDA before withdrawal.
    #[account(executable)]
    pub local_prover_program: UncheckedAccount<'info>,
    /// CHECK: prove-only authority, scoped to the chosen prover's own program ID.
    #[account(address = prove_authority_pda(&local_prover_program.key()).0 @ FlashFulfillerError::InvalidProveAuthority)]
    pub prove_authority: UncheckedAccount<'info>,
    /// CHECK: validated by the prover during CPI.
    pub local_prover_event_authority: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub system_program: Program<'info, System>,
}

/// Proves and withdraws a same-chain reward directly to the solver.
///
/// Include a separate top-level `portal.fulfill` for this intent and solver in
/// the same transaction, before or after this instruction. The guard runs before
/// any CPI. Transaction atomicity ties withdrawal to successful fulfillment,
/// while the separate instruction restores one CPI stack frame for route calls.
///
/// Prepend `ComputeBudgetInstruction::request_heap_frame(256 * 1024)`; the
/// program's custom allocator applies to this instruction too.
///
/// Remaining accounts are `(intent_vault_ata, solver_ata, mint)` triples, one per
/// unique reward mint in `Reward::token_amounts()` order. Create the solver's ATAs
/// before calling; portal validates their ownership and transfers the reward.
/// Native rewards go to the solver directly. After fulfillment the spread stays
/// with the solver; no flash vault, sweep, or intent buffer is involved.
///
/// Fails with `RewardNotPaidInFull` unless the vault holds the full reward and
/// the solver receives all of it, so a reward mint that charges a transfer fee
/// cannot settle here.
pub fn prove_and_withdraw<'info>(
    ctx: Context<'info, ProveAndWithdraw<'info>>,
    args: ProveAndWithdrawArgs,
) -> Result<()> {
    let ProveAndWithdrawArgs { route_hash, reward } = args;
    let intent_hash = types::intent_hash(CHAIN_ID, &route_hash, &reward.hash());
    require_paired_fulfill(
        &ctx.accounts.instructions,
        &intent_hash,
        &ctx.accounts.solver.key(),
    )?;

    let reward_token_amounts = reward.token_amounts()?;
    require!(
        ctx.remaining_accounts.len()
            == reward_token_amounts.len() * VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
        FlashFulfillerError::InvalidRemainingAccounts
    );
    let reward_transfers = VecTokenTransferAccounts::try_from(ctx.remaining_accounts)?.into_inner();
    let owed = reward_transfers
        .iter()
        .map(|transfer| {
            reward_token_amounts
                .get(transfer.mint.key)
                .copied()
                .ok_or_else(|| error!(FlashFulfillerError::InvalidRemainingAccounts))
        })
        .collect::<Result<Vec<_>>>()?;
    require_vault_holds_reward(
        &ctx.accounts.intent_vault,
        reward.native_amount,
        &reward_transfers,
        &owed,
    )?;
    let held = reward_transfers
        .iter()
        .map(|transfer| Ok(transfer.to_data()?.amount))
        .collect::<Result<Vec<_>>>()?;

    ProveWithdraw {
        payer: &ctx.accounts.payer,
        claimant: &ctx.accounts.solver,
        proof: &ctx.accounts.proof,
        intent_vault: &ctx.accounts.intent_vault,
        withdrawn_marker: &ctx.accounts.withdrawn_marker,
        portal_program: &ctx.accounts.portal_program,
        local_prover_program: &ctx.accounts.local_prover_program,
        prove_authority: &ctx.accounts.prove_authority,
        local_prover_event_authority: &ctx.accounts.local_prover_event_authority,
        token_program: &ctx.accounts.token_program,
        token_2022_program: &ctx.accounts.token_2022_program,
        system_program: &ctx.accounts.system_program,
    }
    .execute(intent_hash, route_hash, reward, &reward_transfers)?;

    for ((transfer, owed), held) in reward_transfers.iter().zip(&owed).zip(held) {
        let received = transfer.to_data()?.amount.checked_sub(held);
        require!(
            received.is_some_and(|received| received >= *owed),
            FlashFulfillerError::RewardNotPaidInFull
        );
    }
    Ok(())
}

/// Fails unless the vault holds the full reward. Portal checks `intent_vault`
/// against the intent's vault PDA and each triple's `from` against that vault's
/// ATA when it withdraws, later in this instruction.
fn require_vault_holds_reward(
    intent_vault: &AccountInfo,
    native_amount: u64,
    transfers: &[TokenTransferAccounts],
    owed: &[u64],
) -> Result<()> {
    require!(
        intent_vault.lamports() >= native_amount,
        FlashFulfillerError::RewardNotPaidInFull
    );
    for (transfer, owed) in transfers.iter().zip(owed) {
        require!(
            transfer.from_data()?.amount >= *owed,
            FlashFulfillerError::RewardNotPaidInFull
        );
    }
    Ok(())
}
