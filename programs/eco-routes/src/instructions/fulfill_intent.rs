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
        let expected_destination = spl_associated_token_account::get_associated_token_address(
            &execution_authority.key(),
            &mint_key,
        );

        let mint_account = accounts.next().ok_or(EcoRoutesError::InvalidRouteMint)?;
        let source_account = accounts
            .next()
            .ok_or(EcoRoutesError::InvalidRouteTokenAccount)?;
        let destination_account = accounts
            .next()
            .ok_or(EcoRoutesError::InvalidRouteTokenAccount)?;

        require_keys_eq!(
            mint_account.key(),
            mint_key,
            EcoRoutesError::InvalidRouteMint
        );
        require_keys_eq!(
            destination_account.key(),
            expected_destination,
            EcoRoutesError::InvalidRouteTokenAccount
        );

        let mint = Mint::try_deserialize(&mut &mint_account.data.borrow()[..])?;
        let src_token = TokenAccount::try_deserialize(&mut &source_account.data.borrow()[..])?;

        require_keys_eq!(src_token.mint, mint_key, EcoRoutesError::InvalidRouteMint);
        require_keys_eq!(
            src_token.owner,
            solver.key(),
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
    accounts: &mut Iter<AccountInfo<'info>>,
    route: &mut Route,
    solver: &Signer<'info>,
    execution_authority_bump: u8,
) -> Result<()> {
    for call in &mut route.calls {
        let mut call_data_with_account_metas = SvmCallDataWithAccountMetas {
            svm_call_data: SvmCallData::try_from_slice(&call.calldata)?,
            account_metas: Vec::new(),
        };

        let call_accounts: Vec<_> = accounts
            .by_ref()
            .take(call_data_with_account_metas.svm_call_data.num_account_metas as usize)
            .map(|a| a.to_account_info())
            .collect();

        for acc in &call_accounts {
            let meta = AccountMeta {
                pubkey: if acc.key() == solver.key() {
                    crate::ID
                } else {
                    acc.key()
                },
                is_signer: if acc.key == &execution_authority_key(&route.salt).0 {
                    true
                } else {
                    acc.is_signer
                },
                is_writable: acc.is_writable,
            };
            call_data_with_account_metas.account_metas.push(meta.into());
        }

        call.calldata = call_data_with_account_metas.try_to_vec()?;

        let ix = Instruction {
            program_id: Pubkey::new_from_array(call.destination),
            data: call_data_with_account_metas
                .svm_call_data
                .instruction_data
                .clone(),
            accounts: call_data_with_account_metas
                .account_metas
                .into_iter()
                .map(|m| {
                    if m.pubkey == crate::ID {
                        AccountMeta {
                            pubkey: solver.key(),
                            is_signer: m.is_signer,
                            is_writable: m.is_writable,
                        }
                    } else {
                        m.into()
                    }
                })
                .collect(),
        };

        invoke_signed(
            &ix,
            &call_accounts,
            &[&[
                b"execution_authority",
                route.salt.as_ref(),
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
    #[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
    pub enum MailboxInstruction {
        /// Initializes the program.
        Init(Init),
        /// Processes a message.
        InboxProcess(InboxProcess),
        /// Sets the default ISM.
        InboxSetDefaultIsm(Pubkey),
        /// Gets the recipient's ISM.
        InboxGetRecipientIsm(Pubkey),
        /// Dispatches a message.
        OutboxDispatch(OutboxDispatch),
        /// Gets the number of messages that have been dispatched.
        OutboxGetCount,
        /// Gets the latest checkpoint.
        OutboxGetLatestCheckpoint,
        /// Gets the root of the dispatched message merkle tree.
        OutboxGetRoot,
        /// Gets the owner of the Mailbox.
        GetOwner,
        /// Transfers ownership of the Mailbox.
        TransferOwnership(Option<Pubkey>),
        /// Transfers accumulated protocol fees to the beneficiary.
        ClaimProtocolFees,
        /// Sets the protocol fee configuration.
        SetProtocolFeeConfig,
    }

    /// Instruction data for the Init instruction.
    #[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
    pub struct Init {}

    /// Instruction data for the OutboxDispatch instruction.
    #[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
    pub struct OutboxDispatch {
        /// The sender of the message.
        /// This is required and not implied because a program uses a dispatch authority PDA
        /// to sign the CPI on its behalf. Instruction processing logic prevents a program from
        /// specifying any message sender it wants by requiring the relevant dispatch authority
        /// to sign the CPI.
        pub sender: Pubkey,
        /// The destination domain of the message.
        pub destination_domain: u32,
        /// The remote recipient of the message.
        pub recipient: [u8; 32],
        /// The message body.
        pub message_body: Vec<u8>,
    }

    /// Instruction data for the InboxProcess instruction.
    #[derive(BorshDeserialize, BorshSerialize, Debug, PartialEq)]
    pub struct InboxProcess {}

    let outbox_dispatch = MailboxInstruction::OutboxDispatch(OutboxDispatch {
        sender: ctx.accounts.dispatch_authority.key(),
        destination_domain: route.destination_domain_id,
        recipient: reward.prover,
        message_body: encoding::encode_fulfillment_message(
            &[*intent_hash],
            &[solver.key().to_bytes()],
        ),
    });

    let ix = Instruction {
        program_id: ctx.accounts.mailbox_program.key(),
        accounts: vec![
            AccountMeta::new(ctx.accounts.outbox_pda.key(), false),
            AccountMeta::new_readonly(ctx.accounts.dispatch_authority.key(), true),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(ctx.accounts.spl_noop_program.key(), false),
            AccountMeta::new(ctx.accounts.payer.key(), true),
            AccountMeta::new_readonly(ctx.accounts.unique_message.key(), true),
            AccountMeta::new(ctx.accounts.dispatched_message_pda.key(), false),
        ],
        data: outbox_dispatch.try_to_vec()?,
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
        &[&[b"dispatch_authority", &[ctx.bumps.dispatch_authority]]],
    )?;

    Ok(())
}
