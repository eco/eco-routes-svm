use crate::{
    encoding,
    error::EcoRoutesError,
    hyperlane,
    state::{IntentFulfillmentMarker, Reward, Route},
};
use anchor_lang::{
    prelude::*,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        program::invoke_signed,
    },
};
use anchor_spl::{
    associated_token::spl_associated_token_account,
    token_interface::{transfer_checked, Mint, TokenAccount, TransferChecked},
};
use borsh::{BorshDeserialize, BorshSerialize};
use itertools::multiunzip;

use super::SerializableAccountMeta;

const ACCOUNTS_COUNT_PER_TOKEN: usize = 3; // mint, source, destination

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SvmCallData {
    pub instruction_data: Vec<u8>,
    pub num_account_metas: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SvmCallDataWithAccountMetas {
    pub svm_call_data: SvmCallData,
    pub account_metas: Vec<SerializableAccountMeta>,
}

impl SvmCallDataWithAccountMetas {
    fn new(
        svm_call_data: SvmCallData,
        account_metas: Vec<SerializableAccountMeta>,
    ) -> Result<Self> {
        require!(
            svm_call_data.num_account_metas as usize == account_metas.len(),
            EcoRoutesError::InvalidAccounts
        );

        Ok(Self {
            svm_call_data,
            account_metas,
        })
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct FulfillIntentArgs {
    pub intent_hash: [u8; 32],
    pub claimant: [u8; 32],
    pub route: Route,
    pub reward: Reward,
}

pub fn execution_authority_key(salt: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"execution_authority", salt], &crate::ID)
}

pub fn dispatch_authority_key() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"dispatch_authority"], &crate::ID)
}

#[derive(Accounts)]
#[instruction(args: FulfillIntentArgs)]
pub struct FulfillIntent<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(mut)]
    pub solver: Signer<'info>,

    /// CHECK: Address is enforced
    #[account(
        mut,
        seeds = [b"execution_authority", args.route.salt.as_ref()],
        bump,
    )]
    pub execution_authority: UncheckedAccount<'info>,

    /// CHECK: Address is enforced
    #[account(
        mut,
        seeds = [b"dispatch_authority"],
        bump,
    )]
    pub dispatch_authority: UncheckedAccount<'info>,

    /// CHECK: Address is enforced
    #[account(address = hyperlane::MAILBOX_ID @ EcoRoutesError::NotMailbox)]
    pub mailbox_program: UncheckedAccount<'info>,

    /// CHECK: Checked in CPI
    #[account(mut)]
    pub outbox_pda: UncheckedAccount<'info>,

    /// CHECK: Checked in CPI
    pub spl_noop_program: UncheckedAccount<'info>,

    #[account(mut)]
    pub unique_message: Signer<'info>,

    #[account(
        init,
        payer = payer,
        space = 8 + IntentFulfillmentMarker::INIT_SPACE,
        seeds = [b"intent_fulfillment_marker", args.intent_hash.as_ref()],
        bump,
    )]
    pub intent_fulfillment_marker: Account<'info, IntentFulfillmentMarker>,

    /// CHECK: Checked in CPI
    #[account(mut)]
    pub dispatched_message_pda: UncheckedAccount<'info>,

    pub spl_token_program: Program<'info, anchor_spl::token::Token>,
    pub spl_token_2022_program: Program<'info, anchor_spl::token_2022::Token2022>,

    pub system_program: Program<'info, System>,
}

pub const SOLVER_PLACEHOLDER_PUBKEY: Pubkey =
    pubkey!("So1ver1111111111111111111111111111111111111");

pub fn fulfill_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, FulfillIntent<'info>>,
    args: FulfillIntentArgs,
) -> Result<()> {
    let solver = &ctx.accounts.solver;
    let execution_authority = &ctx.accounts.execution_authority;
    let intent_hash = args.intent_hash;
    let route = args.route;
    let reward = args.reward;

    let (token_accounts, route_calls_accounts) = ctx
        .remaining_accounts
        .split_at_checked(route.tokens.len() * ACCOUNTS_COUNT_PER_TOKEN)
        .ok_or(EcoRoutesError::InvalidAccounts)?;
    let token_accounts = token_accounts
        .chunks_exact(ACCOUNTS_COUNT_PER_TOKEN)
        .map(TryInto::try_into)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| EcoRoutesError::InvalidAccounts)?;

    transfer_route_tokens(
        &token_accounts,
        &route,
        solver,
        execution_authority,
        ctx.accounts.spl_token_program.to_account_info(),
        ctx.accounts.spl_token_2022_program.to_account_info(),
    )?;
    let route = execute_route_calls(
        route_calls_accounts,
        route,
        solver,
        ctx.bumps.execution_authority,
    )?;
    validate_intent_hash(&route, &reward, &intent_hash)?;
    mark_fulfillment(
        &mut ctx.accounts.intent_fulfillment_marker,
        intent_hash,
        ctx.bumps.intent_fulfillment_marker,
    );

    hyperlane::dispatch_fulfillment_message(
        &route,
        &reward,
        &intent_hash,
        args.claimant,
        &ctx.accounts.mailbox_program,
        &ctx.accounts.outbox_pda,
        &ctx.accounts.dispatch_authority,
        &ctx.accounts.spl_noop_program,
        &ctx.accounts.payer,
        &ctx.accounts.unique_message,
        &ctx.accounts.system_program,
        &ctx.accounts.dispatched_message_pda,
        ctx.bumps.dispatch_authority,
    )
}

fn transfer_route_tokens<'info>(
    token_accounts: &[&[AccountInfo<'info>; ACCOUNTS_COUNT_PER_TOKEN]],
    route: &Route,
    solver: &Signer<'info>,
    execution_authority: &AccountInfo<'info>,
    spl_token_program: AccountInfo<'info>,
    spl_token_2022_program: AccountInfo<'info>,
) -> Result<()> {
    require!(
        token_accounts.len() == route.tokens.len(),
        EcoRoutesError::InvalidAccounts
    );

    for (token, [mint_account, source_account, destination_account]) in
        route.tokens.iter().zip(token_accounts.iter())
    {
        let mint_key = Pubkey::new_from_array(token.token);
        let expected_destination =
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &execution_authority.key(),
                &mint_key,
                destination_account.owner,
            );
        let mint = Mint::try_deserialize(&mut &mint_account.data.borrow()[..])?;
        let source_token_account =
            TokenAccount::try_deserialize(&mut &source_account.data.borrow()[..])?;

        require_keys_eq!(
            mint_account.key(),
            mint_key,
            EcoRoutesError::InvalidRouteMint
        );
        require_keys_eq!(
            source_token_account.mint,
            mint_key,
            EcoRoutesError::InvalidRouteMint
        );
        require_keys_eq!(
            source_token_account.owner,
            solver.key(),
            EcoRoutesError::InvalidRouteTokenAccount
        );
        require_keys_eq!(
            destination_account.key(),
            expected_destination,
            EcoRoutesError::InvalidRouteTokenAccount
        );

        let token_program = if mint_account.owner == &spl_token_program.key() {
            spl_token_program.clone()
        } else {
            spl_token_2022_program.clone()
        };

        transfer_checked(
            CpiContext::new(
                token_program,
                TransferChecked {
                    from: source_account.to_account_info(),
                    mint: mint_account.to_account_info(),
                    to: destination_account.to_account_info(),
                    authority: solver.to_account_info(),
                },
            ),
            token.amount,
            mint.decimals,
        )?;
    }

    Ok(())
}

fn execute_route_calls<'info>(
    route_calls_accounts: &[AccountInfo<'info>],
    mut route: Route,
    solver: &Signer<'info>,
    execution_authority_bump: u8,
) -> Result<Route> {
    let mut route_calls_accounts = route_calls_accounts.into_iter();
    let (call_accounts, svm_call_datas, instructions): (Vec<_>, Vec<_>, Vec<_>) = route
        .calls
        .iter()
        .map(|call| {
            let svm_call_data = SvmCallData::try_from_slice(&call.calldata)?;
            let call_accounts: Vec<_> = route_calls_accounts
                .by_ref()
                .take(svm_call_data.num_account_metas as usize)
                .map(ToAccountInfo::to_account_info)
                .collect();
            let expected_account_metas: Vec<SerializableAccountMeta> = call_accounts
                .iter()
                .map(|account| expected_account_meta(account, solver, route.salt))
                .map(Into::into)
                .collect();

            let program_id = Pubkey::new_from_array(call.destination);
            let accounts = call_accounts
                .iter()
                .map(|account| actual_account_meta(account, route.salt))
                .collect();
            let instruction =
                Instruction::new_with_bytes(program_id, &svm_call_data.instruction_data, accounts);

            Ok((
                call_accounts,
                SvmCallDataWithAccountMetas::new(svm_call_data, expected_account_metas)?,
                instruction,
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(multiunzip)?;

    call_accounts
        .iter()
        .zip(instructions)
        .try_for_each(|(call_accounts, instruction)| {
            invoke_signed(
                &instruction,
                &call_accounts,
                &[&[
                    b"execution_authority",
                    route.salt.as_ref(),
                    &[execution_authority_bump],
                ]],
            )
        })?;

    // Replace solver's pubkey with placeholder to reconstruct route data matching the intent hash.
    // We don't know the solver when creating the route on source chain, so we use a placeholder
    // that gets replaced with the actual solver during fulfillment to avoid tx size limits.
    route
        .calls
        .iter_mut()
        .zip(svm_call_datas)
        .try_for_each(|(call, svm_call_data)| {
            call.calldata = svm_call_data.try_to_vec()?;

            Result::<()>::Ok(())
        })?;

    Ok(route)
}

fn expected_account_meta(
    account: &AccountInfo,
    solver: &Signer,
    route_salt: [u8; 32],
) -> AccountMeta {
    AccountMeta {
        pubkey: if account.key() == solver.key() {
            SOLVER_PLACEHOLDER_PUBKEY
        } else {
            account.key()
        },
        is_signer: account.key == &execution_authority_key(&route_salt).0 || account.is_signer,
        is_writable: account.is_writable,
    }
}

fn actual_account_meta(account: &AccountInfo, route_salt: [u8; 32]) -> AccountMeta {
    AccountMeta {
        pubkey: account.key(),
        is_signer: account.key == &execution_authority_key(&route_salt).0 || account.is_signer,
        is_writable: account.is_writable,
    }
}

fn validate_intent_hash(route: &Route, reward: &Reward, expected: &[u8; 32]) -> Result<()> {
    let hash = encoding::get_intent_hash(route, reward);
    require!(hash == *expected, EcoRoutesError::InvalidIntent);

    Ok(())
}

fn mark_fulfillment<'info>(
    marker: &mut Account<'info, IntentFulfillmentMarker>,
    hash: [u8; 32],
    intent_fulfillment_marker_bump: u8,
) {
    marker.intent_hash = hash;
    marker.bump = intent_fulfillment_marker_bump;
}
