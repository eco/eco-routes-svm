use std::ops::Range;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::system_program;
use anchor_spl::token::spl_token;
use anchor_spl::token_2022::spl_token_2022::extension::StateWithExtensions;
use anchor_spl::token_2022::spl_token_2022::state::Account as Token2022Account;
use anchor_spl::{associated_token, token, token_2022};
use eco_svm_std::account::AccountExt;
use eco_svm_std::{Bytes32, CHAIN_ID};
use solana_keccak_hasher::hashv;

use crate::events::IntentFulfilled;
use crate::instructions::fund_context::FundTokenContext;
use crate::instructions::{now, PortalError};
use crate::state::{executor_pda, FulfillMarker, EXECUTOR_SEED, FULFILL_MARKER_SEED};
use crate::types::{
    self, Calldata, CalldataWithAccounts, Route, VecTokenTransferAccounts,
    VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
};

/// Byte offsets of `amount` in the SPL token account layout — the only field a
/// route call may legitimately change. Shared verbatim by token-2022's base
/// account, whose extensions live past byte 165.
const TOKEN_AMOUNT_RANGE: Range<usize> = 64..72;

/// End of the fixed SPL token account layout. Token-2022 extensions live past
/// this offset; adding or growing one requires `Reallocate`, which changes the
/// account's length. Token-2022's base account shares this layout verbatim.
const TOKEN_BASE_LEN: usize = spl_token::state::Account::LEN;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct FulfillArgs {
    pub intent_hash: Bytes32,
    pub route: Route,
    pub reward_hash: Bytes32,
    pub claimant: Bytes32,
}

#[derive(Accounts)]
#[instruction(args: FulfillArgs)]
pub struct Fulfill<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut)]
    pub solver: Signer<'info>,
    /// CHECK: address is validated
    #[account(address = executor_pda().0 @ PortalError::InvalidExecutor)]
    pub executor: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub fulfill_marker: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub associated_token_program: Program<'info, associated_token::AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn fulfill_intent<'info>(ctx: Context<'info, Fulfill<'info>>, args: FulfillArgs) -> Result<()> {
    let FulfillArgs {
        intent_hash: expected_intent_hash,
        route,
        reward_hash,
        claimant,
    } = args;

    require!(route.portal == crate::ID, PortalError::InvalidPortal);
    require!(route.deadline >= now()?, PortalError::RouteExpired);

    let (token_transfer_accounts, call_accounts) = token_transfer_and_call_accounts(&ctx, &route)?;
    fund_executor(&ctx, &route, token_transfer_accounts)?;
    let executor_atas_hash = executor_atas_digest(ctx.accounts.executor.key, call_accounts)?;
    let route = execute_route_calls(ctx.accounts.executor.key, route, call_accounts)?;

    verify_executor_intact(&ctx.accounts.executor)?;
    require!(
        executor_atas_digest(ctx.accounts.executor.key, call_accounts)? == executor_atas_hash,
        PortalError::ExecutorAtaCorrupted
    );

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    require!(
        intent_hash == expected_intent_hash,
        PortalError::InvalidIntentHash
    );
    mark_fulfilled(&ctx, &intent_hash, &claimant, route.deadline)?;

    emit!(IntentFulfilled::new(intent_hash, claimant));

    Ok(())
}

fn token_transfer_and_call_accounts<'info>(
    ctx: &Context<'info, Fulfill<'info>>,
    route: &Route,
) -> Result<(VecTokenTransferAccounts<'info>, &'info [AccountInfo<'info>])> {
    let split_index = route.tokens.len() * VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE;
    require!(
        split_index <= ctx.remaining_accounts.len(),
        PortalError::InvalidTokenTransferAccounts
    );
    let (token_transfer_accounts, call_accounts) = ctx.remaining_accounts.split_at(split_index);

    Ok((token_transfer_accounts.try_into()?, call_accounts))
}

fn fund_executor<'info>(
    ctx: &Context<'info, Fulfill<'info>>,
    route: &Route,
    accounts: VecTokenTransferAccounts<'info>,
) -> Result<()> {
    let route_token_amounts = route.token_amounts()?;
    let funded_tokens = FundTokenContext::from(ctx).fund_tokens(accounts, &route_token_amounts)?;

    require!(
        funded_tokens.iter().eq(route_token_amounts.keys()),
        PortalError::InvalidMint
    );

    if route.native_amount > 0 {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.solver.to_account_info(),
                    to: ctx.accounts.executor.to_account_info(),
                },
            ),
            route.native_amount,
        )?;
    }

    Ok(())
}

fn execute_route_calls(
    executor: &Pubkey,
    mut route: Route,
    call_accounts: &[AccountInfo],
) -> Result<Route> {
    let (_, bump) = executor_pda();
    let signer_seeds = [EXECUTOR_SEED, &[bump]];
    let mut call_accounts = call_accounts.iter();

    route.calls.iter_mut().try_for_each(|call| {
        let calldata = Calldata::try_from_slice(&call.data)?;
        let call_accounts: Vec<_> = call_accounts
            .by_ref()
            .take(calldata.account_count as usize)
            .map(ToAccountInfo::to_account_info)
            .collect();

        execute_route_call(
            executor,
            Pubkey::new_from_array(call.target.into()),
            &calldata.data,
            &call_accounts,
            &signer_seeds,
        )?;

        call.data = borsh::to_vec(&CalldataWithAccounts::new(calldata, call_accounts)?)?;

        Result::Ok(())
    })?;

    Ok(route)
}

fn execute_route_call(
    executor: &Pubkey,
    program_id: Pubkey,
    calldata: &[u8],
    call_accounts: &[AccountInfo],
    signer_seeds: &[&[u8]],
) -> Result<()> {
    let instruction = Instruction::new_with_bytes(
        program_id,
        calldata,
        call_accounts
            .iter()
            .map(|account| AccountMeta {
                pubkey: account.key(),
                is_signer: account.is_signer || account.key() == *executor,
                is_writable: account.is_writable,
            })
            .collect::<Vec<_>>(),
    );

    invoke_signed(&instruction, call_accounts, &[signer_seeds]).map_err(Into::into)
}

/// Lamports are deliberately not part of the invariant: a route that delivers
/// native SOL legitimately drains the executor, and the account carries no
/// rent-exempt state to lose — it is reconstituted by the next `fund_executor`.
fn verify_executor_intact(executor: &UncheckedAccount) -> Result<()> {
    require!(
        executor.owner == &system_program::ID,
        PortalError::ExecutorCorrupted
    );
    require!(executor.data_is_empty(), PortalError::ExecutorCorrupted);

    Ok(())
}

/// State of an executor-owned token account, captured before the route runs.
/// A digest of every token account the executor owns, over everything a route
/// call must not change.
///
/// Folded into a single 32 bytes rather than snapshotted per account: two
/// snapshots are live across the route calls on an allocator that never frees,
/// and per-account vectors overrun the heap on a large route
/// (`flash_fulfill_large_route_consumes_without_oom`).
///
/// Covered: `owner` (a reassigned authority is a permanent per-mint DoS),
/// `delegate` and `close_authority` (which **persist** past the transaction,
/// turning a right that needs an intent authored and fulfilled into a standing
/// one exercisable by a bare transfer), and membership itself — an account that
/// stops parsing, changes owner, or is closed drops out of the fold.
///
/// Executor ATAs must already exist when this runs. `fund_executor` creates one
/// for every mint in `route.tokens`, and any other — an intermediate mint a
/// multi-hop route stages through, say — must be created beforehand: either by a
/// permissionless `create_idempotent` ahead of the `fulfill` instruction, or by
/// declaring the mint in `route.tokens` with `amount: 0`. A route call that
/// creates one itself is rejected, since it joins the fold only on the second
/// pass.
///
/// The extension tail is covered by hashing `len` rather than its bytes: it
/// cannot be hashed directly, because a `Reallocate` moves it outside the
/// caller's input section and the hashing syscall access-violates rather than
/// erroring. `len` is the term that covers an extension being added or grown —
/// keep it.
fn executor_atas_digest(executor: &Pubkey, call_accounts: &[AccountInfo]) -> Result<[u8; 32]> {
    call_accounts.iter().try_fold([0u8; 32], |digest, account| {
        let data = account.try_borrow_data()?;
        let Ok(state) = StateWithExtensions::<Token2022Account>::unpack(&data) else {
            return Ok(digest);
        };
        if state.base.owner != *executor {
            return Ok(digest);
        }

        Ok(hashv(&[
            &digest,
            account.key.as_ref(),
            &data.len().to_le_bytes(),
            &data[..TOKEN_AMOUNT_RANGE.start],
            &data[TOKEN_AMOUNT_RANGE.end..TOKEN_BASE_LEN],
        ])
        .to_bytes())
    })
}

fn mark_fulfilled(
    ctx: &Context<Fulfill>,
    intent_hash: &Bytes32,
    claimant: &Bytes32,
    deadline: u64,
) -> Result<()> {
    let (fulfill_marker, bump) = FulfillMarker::pda(intent_hash);
    require!(
        ctx.accounts.fulfill_marker.key() == fulfill_marker,
        PortalError::InvalidFulfillMarker
    );
    let signer_seeds = [FULFILL_MARKER_SEED, intent_hash.as_ref(), &[bump]];

    FulfillMarker::new(*claimant, ctx.accounts.payer.key(), deadline, bump)
        .init(
            &ctx.accounts.fulfill_marker,
            &ctx.accounts.payer,
            &ctx.accounts.system_program,
            &[&signer_seeds],
        )
        .map_err(|_| PortalError::IntentAlreadyFulfilled.into())
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// `TOKEN_AMOUNT_RANGE` is hand-derived from the SPL token layout, and a
    /// wrong range would hash the wrong bytes while every behavioural test still
    /// passed. Pin it against the packed representation.
    #[test]
    fn token_amount_range_matches_the_layout() {
        let mut packed = [0u8; spl_token::state::Account::LEN];
        spl_token::state::Account {
            amount: u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]),
            ..Default::default()
        }
        .pack_into_slice(&mut packed);

        assert_eq!(packed[TOKEN_AMOUNT_RANGE], [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(packed[..TOKEN_AMOUNT_RANGE.start].iter().all(|b| *b == 0));
        assert!(packed[TOKEN_AMOUNT_RANGE.end..].iter().all(|b| *b == 0));
    }
}
