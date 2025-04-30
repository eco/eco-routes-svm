// programs/eco-routes/src/instructions/fulfill_intent.rs
#![allow(clippy::too_many_arguments)]

use std::slice::Iter;

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
        system_program,
    },
};
use anchor_spl::{
    associated_token::spl_associated_token_account,
    token_interface::{transfer_checked, Mint, TokenAccount, TransferChecked},
};
use borsh::{BorshDeserialize, BorshSerialize};

use super::SerializableAccountMeta;

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SvmCallData {
    pub instruction_data: Vec<u8>,
    pub num_account_metas: u8,
    pub account_metas: Vec<SerializableAccountMeta>,
}

impl SvmCallData {
    pub fn from_calldata_without_account_metas(calldata: &[u8]) -> Result<Self> {
        let mut svm_call_data = Self::try_from_slice(calldata)?;
        if svm_call_data.num_account_metas == 0 {
            return Err(EcoRoutesError::InvalidFulfillCalls.into());
        }
        svm_call_data.account_metas = Vec::new();
        Ok(svm_call_data)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.try_to_vec()?)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct FulfillIntentArgs {
    pub intent_hash: [u8; 32],
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
        address = execution_authority_key(&args.route.salt).0 @ EcoRoutesError::InvalidExecutionAuthority
    )]
    pub execution_authority: UncheckedAccount<'info>,

    /// CHECK: Address is enforced
    #[account(
        seeds = [b"dispatch_authority"],
        bump,
        address = dispatch_authority_key().0 @ EcoRoutesError::InvalidDispatchAuthority
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
        seeds = [b"intent_fulfillment", args.intent_hash.as_ref()],
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

pub fn fulfill_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, FulfillIntent<'info>>,
    args: FulfillIntentArgs,
) -> Result<()> {
    let solver = &ctx.accounts.solver;
    let execution_authority = &ctx.accounts.execution_authority;
    let intent_hash = args.intent_hash;
    let mut route = args.route;
    let reward = args.reward;

    let mut remaining_accounts = ctx.remaining_accounts.iter();

    transfer_route_tokens(
        &mut remaining_accounts,
        &route,
        solver,
        execution_authority,
        ctx.accounts.spl_token_program.to_account_info(),
        ctx.accounts.spl_token_2022_program.to_account_info(),
    )?;

    execute_route_calls(
        &mut remaining_accounts,
        &mut route,
        solver,
        &intent_hash,
        ctx.bumps.execution_authority,
    )?;

    validate_intent_hash(&route, &reward, &intent_hash)?;

    mark_fulfillment(
        &mut ctx.accounts.intent_fulfillment_marker,
        intent_hash,
        ctx.bumps.intent_fulfillment_marker,
    )?;

    dispatch_acknowledgement(&route, &reward, &intent_hash, solver, &ctx)?;

    Ok(())
}

fn transfer_route_tokens<'info>(
    accounts: &mut Iter<AccountInfo<'info>>,
    route: &Route,
    solver: &Signer<'info>,
    execution_authority: &AccountInfo<'info>,
    spl_token_program: AccountInfo<'info>,
    spl_token_2022_program: AccountInfo<'info>,
) -> Result<()> {
    for token in &route.tokens {
        let mint_key = Pubkey::new_from_array(token.token);
        let expected_dest = spl_associated_token_account::get_associated_token_address(
            &execution_authority.key(),
            &mint_key,
        );

        let mint_acc = accounts.next().ok_or(EcoRoutesError::InvalidRouteMint)?;
        let src_acc = accounts
            .next()
            .ok_or(EcoRoutesError::InvalidRouteTokenAccount)?;
        let dest_acc = accounts
            .next()
            .ok_or(EcoRoutesError::InvalidRouteTokenAccount)?;

        require_keys_eq!(mint_acc.key(), mint_key, EcoRoutesError::InvalidRouteMint);
        require_keys_eq!(
            dest_acc.key(),
            expected_dest,
            EcoRoutesError::InvalidRouteTokenAccount
        );

        let mint = Mint::try_deserialize(&mut &mint_acc.data.borrow()[..])?;
        let src_token = TokenAccount::try_deserialize(&mut &src_acc.data.borrow()[..])?;

        require_keys_eq!(src_token.mint, mint_key, EcoRoutesError::InvalidRouteMint);
        require_keys_eq!(
            src_token.owner,
            solver.key(),
            EcoRoutesError::InvalidRouteTokenAccount
        );

        let token_program = if mint_acc.owner == &spl_token_program.key() {
            spl_token_program.clone()
        } else {
            spl_token_2022_program.clone()
        };

        transfer_checked(
            CpiContext::new(
                token_program,
                TransferChecked {
                    from: src_acc.to_account_info(),
                    mint: mint_acc.to_account_info(),
                    to: dest_acc.to_account_info(),
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
    accounts: &mut Iter<AccountInfo<'info>>,
    route: &mut Route,
    solver: &Signer<'info>,
    intent_hash: &[u8; 32],
    execution_authority_bump: u8,
) -> Result<()> {
    for call in &mut route.calls {
        let mut call_data = SvmCallData::from_calldata_without_account_metas(&call.calldata)?;

        let call_accounts: Vec<_> = accounts
            .by_ref()
            .take(call_data.num_account_metas as usize)
            .map(|a| a.to_account_info())
            .collect();

        for acc in &call_accounts {
            let meta = AccountMeta {
                pubkey: if acc.key() == solver.key() {
                    Pubkey::default()
                } else {
                    acc.key()
                },
                is_signer: acc.is_signer,
                is_writable: acc.is_writable,
            };
            call_data.account_metas.push(meta.into());
        }

        call.calldata = call_data.to_bytes()?;

        let ix = Instruction {
            program_id: Pubkey::new_from_array(call.destination),
            data: call_data.instruction_data.clone(),
            accounts: call_data
                .account_metas
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
        };

        invoke_signed(
            &ix,
            &call_accounts,
            &[&[
                b"execution_authority",
                intent_hash,
                &[execution_authority_bump],
            ]],
        )?;
    }
    Ok(())
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
) -> Result<()> {
    marker.intent_hash = hash;
    marker.bump = intent_fulfillment_marker_bump;
    Ok(())
}

fn dispatch_acknowledgement<'info>(
    route: &Route,
    reward: &Reward,
    intent_hash: &[u8; 32],
    solver: &Signer<'info>,
    ctx: &Context<FulfillIntent<'info>>,
) -> Result<()> {
    #[derive(AnchorSerialize, AnchorDeserialize)]
    struct OutboxDispatch {
        sender: Pubkey,
        destination_domain: u32,
        recipient: [u8; 32],
        message_body: Vec<u8>,
    }

    let outbox_dispatch = OutboxDispatch {
        sender: ctx.accounts.dispatch_authority.key(),
        destination_domain: route.destination_domain_id,
        recipient: reward.prover,
        message_body: encoding::encode_fulfillment_message(
            &[*intent_hash],
            &[solver.key().to_bytes()],
        ),
    };

    let mut ix_data = vec![4];
    ix_data.extend(outbox_dispatch.try_to_vec()?);

    let ix = Instruction {
        program_id: ctx.accounts.mailbox_program.key(),
        accounts: vec![
            AccountMeta::new(ctx.accounts.outbox_pda.key(), false),
            AccountMeta::new_readonly(ctx.accounts.dispatch_authority.key(), true),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(ctx.accounts.spl_noop_program.key(), false),
            AccountMeta::new_readonly(ctx.accounts.payer.key(), true),
            AccountMeta::new_readonly(ctx.accounts.unique_message.key(), true),
            AccountMeta::new(ctx.accounts.dispatched_message_pda.key(), false),
        ],
        data: ix_data,
    };

    invoke_signed(
        &ix,
        &[
            ctx.accounts.outbox_pda.to_account_info(),
            ctx.accounts.dispatch_authority.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.spl_noop_program.to_account_info(),
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.unique_message.to_account_info(),
            ctx.accounts.dispatched_message_pda.to_account_info(),
        ],
        &[&[
            b"hyperlane",
            b"-",
            b"dispatch_authority",
            &[ctx.bumps.dispatch_authority],
        ]],
    )?;

    Ok(())
}
