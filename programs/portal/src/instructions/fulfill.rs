use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::system_program;
use anchor_spl::{associated_token, token, token_2022};
use eco_svm_std::account::AccountExt;
use eco_svm_std::{Bytes32, CHAIN_ID};

use crate::events::IntentFulfilled;
use crate::instructions::fund_context::FundTokenContext;
use crate::instructions::PortalError;
use crate::state::{executor_pda, FulfillMarker, EXECUTOR_SEED, FULFILL_MARKER_SEED};
use crate::types::{
    self, Calldata, CalldataWithAccounts, Route, VecTokenTransferAccounts,
    VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
};

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

pub fn fulfill_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, Fulfill<'info>>,
    args: FulfillArgs,
) -> Result<()> {
    let FulfillArgs {
        intent_hash: expected_intent_hash,
        route,
        reward_hash,
        claimant,
    } = args;

    require!(route.portal == crate::ID, PortalError::InvalidPortal);
    require!(
        route.deadline
            >= Clock::get()?
                .unix_timestamp
                .try_into()
                .expect("timestamp must fit in u64"),
        PortalError::RouteExpired
    );

    msg!(
        "portal.fulfill: start tokens={} calls={} native_amount={} remaining_accounts={}",
        route.tokens.len(),
        route.calls.len(),
        route.native_amount,
        ctx.remaining_accounts.len()
    );
    let (token_transfer_accounts, call_accounts) = token_transfer_and_call_accounts(&ctx, &route)?;
    msg!(
        "portal.fulfill: split_accounts token_transfer_accounts={} call_accounts={}",
        route.tokens.len() * VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
        call_accounts.len()
    );
    msg!("portal.fulfill: before_fund_executor");
    fund_executor(&ctx, &route, token_transfer_accounts)?;
    msg!("portal.fulfill: after_fund_executor");
    msg!("portal.fulfill: before_execute_route_calls");
    let route = execute_route_calls(ctx.accounts.executor.key, route, call_accounts)?;
    msg!("portal.fulfill: after_execute_route_calls");

    msg!("portal.fulfill: before_route_hash");
    let route_hash = route.hash();
    msg!("portal.fulfill: after_route_hash");
    let intent_hash = types::intent_hash(CHAIN_ID, &route_hash, &reward_hash);
    require!(
        intent_hash == expected_intent_hash,
        PortalError::InvalidIntentHash
    );
    msg!("portal.fulfill: before_mark_fulfilled");
    mark_fulfilled(&ctx, &intent_hash, &claimant)?;
    msg!("portal.fulfill: after_mark_fulfilled");

    emit!(IntentFulfilled::new(intent_hash, claimant));

    Ok(())
}

fn token_transfer_and_call_accounts<'c, 'info>(
    ctx: &Context<'_, '_, 'c, 'info, Fulfill<'info>>,
    route: &Route,
) -> Result<(VecTokenTransferAccounts<'info>, &'c [AccountInfo<'info>])> {
    let split_index = route.tokens.len() * VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE;
    require!(
        split_index <= ctx.remaining_accounts.len(),
        PortalError::InvalidTokenTransferAccounts
    );
    let (token_transfer_accounts, call_accounts) = ctx.remaining_accounts.split_at(split_index);

    Ok((token_transfer_accounts.try_into()?, call_accounts))
}

fn fund_executor<'info>(
    ctx: &Context<'_, '_, '_, 'info, Fulfill<'info>>,
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
                ctx.accounts.system_program.to_account_info(),
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

    route
        .calls
        .iter_mut()
        .enumerate()
        .try_for_each(|(index, call)| {
            msg!(
                "portal.exec: call={} before_calldata_decode compact_len={}",
                index,
                call.data.len()
            );
            let calldata = Calldata::try_from_slice(&call.data)?;
            let target = Pubkey::new_from_array(call.target.into());
            msg!(
                "portal.exec: call={} decoded account_count={} data_len={}",
                index,
                calldata.account_count,
                calldata.data.len()
            );
            let call_accounts: Vec<_> = call_accounts
                .by_ref()
                .take(calldata.account_count as usize)
                .map(ToAccountInfo::to_account_info)
                .collect();
            msg!(
                "portal.exec: call={} collected_accounts={}",
                index,
                call_accounts.len()
            );

            msg!("portal.exec: call={} before_invoke", index);
            execute_route_call(
                executor,
                target,
                &calldata.data,
                &call_accounts,
                &signer_seeds,
            )?;
            msg!("portal.exec: call={} after_invoke", index);

            msg!("portal.exec: call={} before_rebuild_full_calldata", index);
            let calldata_with_accounts = CalldataWithAccounts::new(calldata, call_accounts)?;
            msg!("portal.exec: call={} after_rebuild_full_calldata", index);
            msg!("portal.exec: call={} before_serialize_full_calldata", index);
            call.data = calldata_with_accounts.try_to_vec()?;
            msg!(
                "portal.exec: call={} after_serialize_full_calldata len={}",
                index,
                call.data.len()
            );

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

fn mark_fulfilled(ctx: &Context<Fulfill>, intent_hash: &Bytes32, claimant: &Bytes32) -> Result<()> {
    let (fulfill_marker, bump) = FulfillMarker::pda(intent_hash);
    require!(
        ctx.accounts.fulfill_marker.key() == fulfill_marker,
        PortalError::InvalidFulfillMarker
    );
    let signer_seeds = [FULFILL_MARKER_SEED, intent_hash.as_ref(), &[bump]];

    FulfillMarker::new(*claimant, bump)
        .init(
            &ctx.accounts.fulfill_marker,
            &ctx.accounts.payer,
            &ctx.accounts.system_program,
            &[&signer_seeds],
        )
        .map_err(|_| PortalError::IntentAlreadyFulfilled.into())
}
