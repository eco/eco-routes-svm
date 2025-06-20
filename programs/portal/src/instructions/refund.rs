use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::token_interface::{close_account, CloseAccount};
use anchor_spl::{token, token_2022};
use eco_svm_std::{Bytes32, Proof};

use crate::events::IntentRefunded;
use crate::instructions::PortalError;
use crate::state::{self, VAULT_SEED};
use crate::types::{self, Reward, TokenTransferAccounts, VecTokenTransferAccounts};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RefundArgs {
    pub destination_chain: Bytes32,
    pub route_hash: Bytes32,
    pub reward: Reward,
}

#[derive(Accounts)]
#[instruction(args: RefundArgs)]
pub struct Refund<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address is validated
    #[account(mut, address = args.reward.creator @ PortalError::InvalidCreator)]
    pub creator: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(mut, address = state::vault_pda(&types::intent_hash(&args.destination_chain, &args.route_hash, &args.reward)).0 @ PortalError::InvalidVault)]
    pub vault: UncheckedAccount<'info>,
    /// CHECK: address is validated
    #[account(address = Proof::pda(&types::intent_hash(&args.destination_chain, &args.route_hash, &args.reward), &args.reward.prover).0 @ PortalError::InvalidProof)]
    pub proof: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub system_program: Program<'info, System>,
}

pub fn refund_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, Refund<'info>>,
    args: RefundArgs,
) -> Result<()> {
    let RefundArgs {
        destination_chain,
        route_hash,
        reward,
    } = args;
    let intent_hash = types::intent_hash(&destination_chain, &route_hash, &reward);
    let (_, bump) = state::vault_pda(&intent_hash);
    let signer_seeds = [VAULT_SEED, intent_hash.as_ref(), &[bump]];

    validate_proof(&ctx.accounts.proof.to_account_info(), destination_chain)?;
    require!(
        reward.deadline <= Clock::get()?.unix_timestamp,
        PortalError::RewardNotExpired
    );

    refund_native(&ctx, &signer_seeds, &reward.creator)?;
    refund_tokens(&ctx, &signer_seeds, ctx.remaining_accounts.try_into()?)?;

    emit!(IntentRefunded::new(intent_hash, reward.creator));

    Ok(())
}

fn validate_proof(proof: &AccountInfo, destination_chain: Bytes32) -> Result<()> {
    if proof.data_is_empty() {
        return Ok(());
    }

    let proof = Proof::deserialize(&mut &proof.data.borrow()[8..])?;
    if proof.destination_chain != destination_chain {
        return Ok(());
    }

    Err(PortalError::IntentAlreadyFulfilled.into())
}

fn refund_native(ctx: &Context<Refund>, signer_seeds: &[&[u8]], creator: &Pubkey) -> Result<()> {
    invoke_signed(
        &system_instruction::transfer(
            &ctx.accounts.vault.key(),
            creator,
            ctx.accounts.vault.lamports(),
        ),
        &[
            ctx.accounts.vault.to_account_info(),
            ctx.accounts.creator.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
        &[signer_seeds],
    )
    .map_err(Into::into)
}

fn refund_tokens<'info>(
    ctx: &Context<'_, '_, '_, 'info, Refund<'info>>,
    signer_seeds: &[&[u8]],
    accounts: VecTokenTransferAccounts<'info>,
) -> Result<()> {
    accounts
        .into_inner()
        .into_iter()
        .try_for_each(|fund_token_accounts| refund_token(ctx, signer_seeds, fund_token_accounts))
}

fn refund_token<'info>(
    ctx: &Context<'_, '_, '_, 'info, Refund<'info>>,
    signer_seeds: &[&[u8]],
    accounts: TokenTransferAccounts<'info>,
) -> Result<()> {
    require!(
        accounts.to_data()?.owner == ctx.accounts.creator.key(),
        PortalError::InvalidCreatorToken
    );

    let token_program = accounts.token_program(
        &ctx.accounts.token_program,
        &ctx.accounts.token_2022_program,
    )?;

    accounts.transfer_with_signer(
        &token_program,
        &ctx.accounts.vault,
        &[signer_seeds],
        accounts.from_data()?.amount,
    )?;

    close_account(CpiContext::new_with_signer(
        token_program,
        CloseAccount {
            account: accounts.from.to_account_info(),
            destination: ctx.accounts.payer.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        },
        &[signer_seeds],
    ))
}
