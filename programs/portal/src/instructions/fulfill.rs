use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::{associated_token, token, token_2022};
use eco_svm_std::account::AccountExt;
use eco_svm_std::{is_prover, Bytes32, CHAIN_ID};

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
    pub route: Route,
    pub reward_hash: Bytes32,
    pub claimant: Pubkey,
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
        route,
        reward_hash,
        claimant,
    } = args;
    let (token_transfer_accounts, call_accounts) = token_transfer_and_call_accounts(&ctx, &route)?;
    fund_executor(&ctx, &route, token_transfer_accounts)?;
    let route = execute_route_calls(ctx.accounts.executor.key, route, call_accounts)?;

    let intent_hash = types::intent_hash(&CHAIN_ID.into(), &route.hash(), &reward_hash);
    mark_fulfilled(&ctx, &intent_hash, &claimant)?;

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

        call.data = CalldataWithAccounts::new(calldata, call_accounts)?.try_to_vec()?;

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
    require!(!is_prover(&program_id), PortalError::InvalidFulfillTarget);

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

fn mark_fulfilled(ctx: &Context<Fulfill>, intent_hash: &Bytes32, claimant: &Pubkey) -> Result<()> {
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
            &signer_seeds,
        )
        .map_err(|_| PortalError::IntentAlreadyFulfilled.into())
}
