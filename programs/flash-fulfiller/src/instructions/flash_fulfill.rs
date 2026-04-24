use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::associated_token::{
    self, get_associated_token_address_with_program_id, AssociatedToken,
};
use anchor_spl::token_interface::{close_account, CloseAccount};
use anchor_spl::{token, token_2022};
use eco_svm_std::prover::{self, IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::{Bytes32, CHAIN_ID};
use portal::instructions::{FulfillArgs, WithdrawArgs};
use portal::types::{
    self, Reward, Route, TokenTransferAccounts, VecTokenTransferAccounts,
    VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE,
};
use spl_pod::option::Nullable;

use crate::cpi;
use crate::events::FlashFulfilled;
use crate::instructions::FlashFulfillerError;
use crate::state::{flash_vault_pda, FlashFulfillIntentAccount, FLASH_VAULT_SEED};

struct FlashFulfillAccounts<'a, 'info> {
    reward: Vec<TokenTransferAccounts<'info>>,
    route: Vec<TokenTransferAccounts<'info>>,
    claimant: Vec<TokenTransferAccounts<'info>>,
    calls: &'a [AccountInfo<'info>],
}

/// How the intent is supplied to `flash_fulfill`: either inline or by hash,
/// referencing a buffer previously written via `set_flash_fulfill_intent`.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub enum FlashFulfillIntent {
    /// Read `(route, reward)` from the buffered `FlashFulfillIntentAccount` for this intent hash.
    IntentHash(Bytes32),
    /// Inline `(route, reward)` pair; no buffer account is required.
    Intent { route: Route, reward: Reward },
}

/// Args for [`flash_fulfill`]: the intent to fulfill, inline or by hash.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct FlashFulfillArgs {
    /// Intent selector — see [`FlashFulfillIntent`].
    pub intent: FlashFulfillIntent,
}

/// Accounts for [`flash_fulfill`].
#[event_cpi]
#[derive(Accounts)]
pub struct FlashFulfill<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address is validated
    #[account(mut, address = flash_vault_pda().0 @ FlashFulfillerError::InvalidFlashVault)]
    pub flash_vault: UncheckedAccount<'info>,
    /// CHECK: only read for the IntentHash variant; address validated in handler
    #[account(mut)]
    pub flash_fulfill_intent: Option<Account<'info, FlashFulfillIntentAccount>>,
    /// CHECK: caller-supplied recipient of leftovers
    #[account(mut)]
    pub claimant: UncheckedAccount<'info>,
    /// CHECK: address validated + initialized via local_prover.prove CPI
    #[account(mut)]
    pub proof: UncheckedAccount<'info>,
    /// CHECK: validated by portal.withdraw against vault_pda(intent_hash)
    #[account(mut)]
    pub intent_vault: UncheckedAccount<'info>,
    /// CHECK: validated by portal.withdraw against WithdrawnMarker::pda(intent_hash)
    #[account(mut)]
    pub withdrawn_marker: UncheckedAccount<'info>,
    /// CHECK: validated by portal.withdraw against proof_closer_pda()
    pub proof_closer: UncheckedAccount<'info>,
    /// CHECK: validated by portal.fulfill against executor_pda()
    #[account(mut)]
    pub executor: UncheckedAccount<'info>,
    /// CHECK: validated by portal.fulfill against FulfillMarker::pda(intent_hash)
    #[account(mut)]
    pub fulfill_marker: UncheckedAccount<'info>,
    /// CHECK: executable-only. Program ID is not validated because it's
    /// deploy-keypair-dependent and passing a malicious look-alike can only
    /// drain `payer`'s SOL (via CPI signer inheritance) — no third-party
    /// harm. Assumes `payer` is the solver/caller, not a sponsored relayer.
    #[account(executable)]
    pub portal_program: UncheckedAccount<'info>,
    /// CHECK: executable-only. See `portal_program` above — same rationale.
    #[account(executable)]
    pub local_prover_program: UncheckedAccount<'info>,
    /// CHECK: local_prover's event authority PDA, validated by local_prover during CPI
    pub local_prover_event_authority: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Executes the full prove → withdraw → fulfill → sweep sequence atomically.
///
/// Remaining accounts layout: reward 3-tuples (intent_vault_ata, flash_vault_ata, mint)
/// × reward.tokens.len(), then route 3-tuples (flash_vault_ata, executor_ata, mint)
/// × route.tokens.len(), then one claimant ATA per reward mint, then any accounts
/// referenced by `route.calls`.
///
/// When invoked via `FlashFulfillIntent::IntentHash`, the consumed
/// `FlashFulfillIntentAccount` buffer is closed at the end and its rent
/// refunded to `payer`.
pub fn flash_fulfill<'info>(
    ctx: Context<'_, '_, '_, 'info, FlashFulfill<'info>>,
    args: FlashFulfillArgs,
) -> Result<()> {
    let FlashFulfillArgs { intent } = args;

    require!(
        ctx.accounts.claimant.key().is_some(),
        FlashFulfillerError::InvalidClaimant
    );

    let close_flash_fulfill_intent = matches!(intent, FlashFulfillIntent::IntentHash(_));
    let (route, reward) = resolve_intent(&ctx, intent)?;
    let route_hash = route.hash();
    let reward_hash = reward.hash();
    let intent_hash = types::intent_hash(CHAIN_ID, &route_hash, &reward_hash);
    let flash_vault = ctx.accounts.flash_vault.key();
    let (_, flash_vault_bump) = flash_vault_pda();
    let flash_vault_seeds: &[&[u8]] = &[FLASH_VAULT_SEED, &[flash_vault_bump]];

    prover::prove(
        &ctx.accounts.local_prover_program.to_account_info(),
        &ctx.accounts.flash_vault.to_account_info(),
        flash_vault_seeds,
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
                    claimant: flash_vault.to_bytes().into(),
                }],
            },
            data: vec![],
        },
    )?;

    let FlashFulfillAccounts {
        reward: reward_transfers,
        route: route_transfers,
        claimant: claimant_transfers,
        calls,
    } = extract_flash_fulfill_accounts(&ctx, reward.tokens.len(), route.tokens.len())?;

    cpi::withdraw::withdraw_intent(
        &ctx.accounts.portal_program.to_account_info(),
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.flash_vault.to_account_info(),
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
    )?;

    let route = strip_call_accounts(route)?;

    cpi::fulfill::fulfill_intent(
        &ctx.accounts.portal_program.to_account_info(),
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.flash_vault.to_account_info(),
        flash_vault_seeds,
        &ctx.accounts.executor.to_account_info(),
        &ctx.accounts.fulfill_marker.to_account_info(),
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.token_2022_program.to_account_info(),
        &ctx.accounts.associated_token_program.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        &route_transfers,
        calls,
        FulfillArgs {
            intent_hash,
            route,
            reward_hash,
            claimant: ctx.accounts.claimant.key().to_bytes().into(),
        },
    )?;

    sweep_leftover_tokens(&ctx, &claimant_transfers, flash_vault_seeds)?;
    let native_fee = sweep_leftover_native(&ctx, flash_vault_seeds)?;

    if close_flash_fulfill_intent {
        ctx.accounts
            .flash_fulfill_intent
            .close(ctx.accounts.payer.to_account_info())?;
    }

    emit_cpi!(FlashFulfilled {
        intent_hash,
        claimant: ctx.accounts.claimant.key(),
        native_fee,
    });

    Ok(())
}

fn extract_flash_fulfill_accounts<'a, 'info>(
    ctx: &'a Context<'_, '_, '_, 'info, FlashFulfill<'info>>,
    reward_token_count: usize,
    route_token_count: usize,
) -> Result<FlashFulfillAccounts<'a, 'info>> {
    let reward_len = reward_token_count * VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE;
    let route_len = route_token_count * VEC_TOKEN_TRANSFER_ACCOUNTS_CHUNK_SIZE;
    require!(
        ctx.remaining_accounts.len() >= reward_len + route_len + reward_token_count,
        FlashFulfillerError::InvalidRemainingAccounts
    );

    let (reward_slice, rest) = ctx.remaining_accounts.split_at(reward_len);
    let (route_slice, rest) = rest.split_at(route_len);
    let (claimant_atas, calls) = rest.split_at(reward_token_count);

    let reward = VecTokenTransferAccounts::try_from(reward_slice)?.into_inner();
    init_flash_vault_reward_atas(ctx, &reward)?;

    let route = VecTokenTransferAccounts::try_from(route_slice)?.into_inner();
    let claimant = reward
        .iter()
        .zip(claimant_atas.iter())
        .map(|(reward_transfer, claimant_ata)| TokenTransferAccounts {
            from: reward_transfer.to.to_account_info(),
            to: claimant_ata.to_account_info(),
            mint: reward_transfer.mint.to_account_info(),
        })
        .collect();

    Ok(FlashFulfillAccounts {
        reward,
        route,
        claimant,
        calls,
    })
}

fn init_flash_vault_reward_atas<'info>(
    ctx: &Context<'_, '_, '_, 'info, FlashFulfill<'info>>,
    reward_transfers: &[TokenTransferAccounts<'info>],
) -> Result<()> {
    reward_transfers
        .iter()
        .try_for_each(|transfer| init_flash_vault_reward_ata(ctx, transfer))
}

fn init_flash_vault_reward_ata<'info>(
    ctx: &Context<'_, '_, '_, 'info, FlashFulfill<'info>>,
    transfer: &TokenTransferAccounts<'info>,
) -> Result<()> {
    let token_program = transfer.token_program(
        &ctx.accounts.token_program,
        &ctx.accounts.token_2022_program,
    )?;

    associated_token::create_idempotent(CpiContext::new(
        ctx.accounts.associated_token_program.to_account_info(),
        associated_token::Create {
            payer: ctx.accounts.payer.to_account_info(),
            associated_token: transfer.to.to_account_info(),
            authority: ctx.accounts.flash_vault.to_account_info(),
            mint: transfer.mint.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            token_program,
        },
    ))
}

/// Strip the trailing `accounts: Vec<SerializableAccountMeta>` from each
/// call's Borsh-encoded `CalldataWithAccounts`, leaving only the `Calldata`
/// prefix in place. The on-wire layout is
/// `[data.len u32][data bytes][account_count u8][accounts.len u32][accounts]`
/// so stripping is a bounds-checked `truncate(4 + data_len + 1)` — no
/// deserialize/reserialize, no intermediate `Vec` allocations.
///
/// This matters for heap pressure: in `flash_fulfill`, decoding then
/// reserializing every call permanently retains ~3× the call size in the
/// Solana bump allocator (which never frees), pushing deep CPI chains into OOM.
fn strip_call_accounts(mut route: Route) -> Result<Route> {
    for call in route.calls.iter_mut() {
        require!(
            call.data.len() >= 5,
            FlashFulfillerError::InvalidCallData
        );
        let data_len = u32::from_le_bytes([
            call.data[0],
            call.data[1],
            call.data[2],
            call.data[3],
        ]) as usize;
        let prefix_len = 4usize
            .checked_add(data_len)
            .and_then(|n| n.checked_add(1))
            .ok_or(FlashFulfillerError::InvalidCallData)?;
        require!(
            prefix_len <= call.data.len(),
            FlashFulfillerError::InvalidCallData
        );
        call.data.truncate(prefix_len);
    }
    Ok(route)
}

fn sweep_leftover_tokens<'info>(
    ctx: &Context<'_, '_, '_, 'info, FlashFulfill<'info>>,
    claimant_transfers: &[TokenTransferAccounts<'info>],
    flash_vault_seeds: &[&[u8]],
) -> Result<()> {
    claimant_transfers.iter().try_for_each(|transfer| {
        let expected_claimant_ata = get_associated_token_address_with_program_id(
            &ctx.accounts.claimant.key(),
            &transfer.mint.key(),
            transfer.token_program_id(),
        );
        require!(
            transfer.to.key() == expected_claimant_ata,
            FlashFulfillerError::InvalidClaimantToken
        );
        require!(
            transfer.to_data()?.owner == ctx.accounts.claimant.key(),
            FlashFulfillerError::InvalidClaimantToken
        );

        let token_program = transfer.token_program(
            &ctx.accounts.token_program,
            &ctx.accounts.token_2022_program,
        )?;
        let leftover = transfer.from_data()?.amount;

        transfer.transfer_with_signer(
            &token_program,
            &ctx.accounts.flash_vault,
            &[flash_vault_seeds],
            leftover,
        )?;

        close_account(CpiContext::new_with_signer(
            token_program,
            CloseAccount {
                account: transfer.from.to_account_info(),
                destination: ctx.accounts.payer.to_account_info(),
                authority: ctx.accounts.flash_vault.to_account_info(),
            },
            &[flash_vault_seeds],
        ))
    })
}

fn sweep_leftover_native<'info>(
    ctx: &Context<'_, '_, '_, 'info, FlashFulfill<'info>>,
    flash_vault_seeds: &[&[u8]],
) -> Result<u64> {
    let leftover = ctx.accounts.flash_vault.lamports();
    if leftover == 0 {
        return Ok(0);
    }

    invoke_signed(
        &system_instruction::transfer(
            &ctx.accounts.flash_vault.key(),
            &ctx.accounts.claimant.key(),
            leftover,
        ),
        &[
            ctx.accounts.flash_vault.to_account_info(),
            ctx.accounts.claimant.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
        &[flash_vault_seeds],
    )?;

    Ok(leftover)
}

fn resolve_intent(
    ctx: &Context<FlashFulfill>,
    intent: FlashFulfillIntent,
) -> Result<(Route, Reward)> {
    match intent {
        FlashFulfillIntent::Intent { route, reward } => Ok((route, reward)),
        FlashFulfillIntent::IntentHash(intent_hash) => {
            let buffer = ctx
                .accounts
                .flash_fulfill_intent
                .as_ref()
                .ok_or(FlashFulfillerError::InvalidFlashFulfillIntentAccount)?;

            require!(
                buffer.key() == FlashFulfillIntentAccount::pda(&intent_hash, &buffer.writer).0,
                FlashFulfillerError::InvalidFlashFulfillIntentAccount
            );
            require!(buffer.finalized, FlashFulfillerError::BufferNotFinalized);

            let route = Route::try_from_slice(&buffer.route_bytes)
                .map_err(|_| FlashFulfillerError::RouteDecodeFailed)?;

            Ok((route, buffer.reward.clone()))
        }
    }
}
