use anchor_lang::prelude::*;
use anchor_spl::{token, token_2022};
use eco_svm_std::prover::{self, IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::{Bytes32, CHAIN_ID};
use portal::instructions::WithdrawArgs;
use portal::types::{
    self, Reward, VecTokenTransferAccounts, VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
};

use crate::cpi;
use crate::instructions::FlashFulfillerError;
use crate::paired_fulfill::require_paired_fulfill;
use crate::state::{prove_authority_pda, PROVE_AUTHORITY_SEED};

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
    /// CHECK: validated by portal.withdraw against proof_closer_pda(reward.prover).
    pub proof_closer: UncheckedAccount<'info>,
    /// CHECK: pinned so the withdraw leg must execute the trusted portal.
    #[account(executable, address = portal::ID @ FlashFulfillerError::InvalidPortalProgram)]
    pub portal_program: UncheckedAccount<'info>,
    /// CHECK: executable-only, as in flash_fulfill (local-prover depends on us).
    /// The signed prove authority is scoped to this program, and portal validates
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

    require!(
        ctx.remaining_accounts.len()
            == reward.token_amounts()?.len() * VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
        FlashFulfillerError::InvalidRemainingAccounts
    );
    let reward_transfers = VecTokenTransferAccounts::try_from(ctx.remaining_accounts)?.into_inner();
    let local_prover = ctx.accounts.local_prover_program.key();
    let (_, bump) = prove_authority_pda(&local_prover);
    let seeds: &[&[u8]] = &[PROVE_AUTHORITY_SEED, local_prover.as_ref(), &[bump]];

    prover::prove(
        &ctx.accounts.local_prover_program.to_account_info(),
        &ctx.accounts.prove_authority.to_account_info(),
        seeds,
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        &ctx.accounts.local_prover_event_authority.to_account_info(),
        &ctx.accounts.proof.to_account_info(),
        ProveArgs {
            domain_id: CHAIN_ID,
            proof_data: ProofData {
                destination: CHAIN_ID,
                intent_hashes_claimants: vec![IntentHashClaimant {
                    intent_hash,
                    claimant: ctx.accounts.solver.key().to_bytes().into(),
                }],
            },
            data: vec![],
        },
    )?;

    cpi::withdraw::withdraw_intent(
        &ctx.accounts.portal_program.to_account_info(),
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.solver.to_account_info(),
        &ctx.accounts.intent_vault.to_account_info(),
        &ctx.accounts.proof.to_account_info(),
        &ctx.accounts.proof_closer.to_account_info(),
        &ctx.accounts.local_prover_program.to_account_info(),
        &ctx.accounts.withdrawn_marker.to_account_info(),
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.token_2022_program.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        &reward_transfers,
        WithdrawArgs {
            destination: CHAIN_ID,
            route_hash,
            reward,
        },
    )
}
