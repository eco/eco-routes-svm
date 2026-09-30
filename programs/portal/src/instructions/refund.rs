use std::collections::BTreeSet;

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token_interface::{close_account, CloseAccount};
use anchor_spl::{token, token_2022};
use eco_svm_std::prover::{cpi, GetProofArgs, Proof};
use eco_svm_std::Bytes32;

use crate::events::IntentRefunded;
use crate::instructions::{now, PortalError};
use crate::state::{vault_pda, WithdrawnMarker, VAULT_SEED};
use crate::types::{self, Reward, TokenTransferAccounts, VecTokenTransferAccounts};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RefundArgs {
    pub destination: u64,
    pub route_hash: Bytes32,
    pub reward: Reward,
    pub prover_data: Vec<u8>,
    pub prover_account_count: u8,
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
    #[account(mut)]
    pub vault: UncheckedAccount<'info>,
    /// CHECK: bound to the committed program; executable status is checked for CPI paths.
    #[account(address = args.reward.prover @ PortalError::InvalidProver)]
    pub prover: Option<UncheckedAccount<'info>>,
    /// CHECK: address is validated
    #[account(mut)]
    pub withdrawn_marker: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub system_program: Program<'info, System>,
}

pub fn refund_intent<'info>(ctx: Context<'info, Refund<'info>>, args: RefundArgs) -> Result<()> {
    let RefundArgs {
        destination,
        route_hash,
        reward,
        prover_data,
        prover_account_count,
    } = args;
    let intent_hash = types::intent_hash(destination, &route_hash, &reward.hash());
    let (vault_pda, bump) = vault_pda(&intent_hash);
    let signer_seeds = [VAULT_SEED, intent_hash.as_ref(), &[bump]];

    require!(
        ctx.accounts.vault.key() == vault_pda,
        PortalError::InvalidVault
    );

    let (token_transfer_accounts, prover_accounts) =
        token_transfer_and_prover_accounts(&ctx, prover_account_count)?;
    authorize_refund(
        &ctx,
        &reward,
        destination,
        GetProofArgs::new(intent_hash, prover_data),
        prover_accounts,
        &token_transfer_accounts,
    )?;

    refund_native(&ctx, &signer_seeds)?;
    refund_tokens(&ctx, &signer_seeds, token_transfer_accounts)?;

    emit!(IntentRefunded::new(intent_hash, reward.creator));

    Ok(())
}

impl<'info> Refund<'info> {
    fn get_proof(
        &self,
        accounts: &[AccountInfo<'info>],
        args: GetProofArgs,
    ) -> Result<Option<Proof>> {
        let prover = self.prover.as_ref().ok_or(PortalError::InvalidProver)?;
        if !prover.executable {
            require!(
                accounts.is_empty() && args.data.is_empty(),
                PortalError::InvalidProof
            );

            return Ok(None);
        }

        cpi::get_proof(prover, accounts, args)
    }
}

fn authorize_refund<'info>(
    ctx: &Context<'info, Refund<'info>>,
    reward: &Reward,
    destination: u64,
    query: GetProofArgs,
    prover_accounts: &[AccountInfo<'info>],
    token_transfer_accounts: &VecTokenTransferAccounts,
) -> Result<()> {
    if WithdrawnMarker::exists(&ctx.accounts.withdrawn_marker, &query.intent_hash)? {
        require!(
            prover_accounts.is_empty() && query.data.is_empty(),
            PortalError::InvalidProof
        );

        return Ok(());
    }

    let proof = ctx.accounts.get_proof(prover_accounts, query)?;
    match proof {
        Some(proof) => {
            require!(proof.destination == destination, PortalError::InvalidProof);
            require!(
                proof.is_cancelled(),
                PortalError::IntentFulfilledAndNotWithdrawn
            );

            if reward.deadline > now()? {
                require_reward_mints_swept(ctx, reward, token_transfer_accounts)?;
            }
        }
        None => {
            require!(reward.deadline <= now()?, PortalError::RewardNotExpired);
        }
    }

    Ok(())
}

fn require_reward_mints_swept(
    ctx: &Context<Refund>,
    reward: &Reward,
    accounts: &VecTokenTransferAccounts,
) -> Result<()> {
    let swept_mints = accounts
        .iter()
        .filter(|accounts| {
            accounts.from.key()
                == get_associated_token_address_with_program_id(
                    ctx.accounts.vault.key,
                    accounts.mint.key,
                    accounts.token_program_id(),
                )
        })
        .map(|accounts| accounts.mint.key())
        .collect::<BTreeSet<_>>();

    require!(
        reward
            .token_amounts()?
            .keys()
            .all(|mint| swept_mints.contains(mint)),
        PortalError::InvalidMint
    );

    Ok(())
}

fn token_transfer_and_prover_accounts<'info>(
    ctx: &Context<'info, Refund<'info>>,
    prover_account_count: u8,
) -> Result<(VecTokenTransferAccounts<'info>, &'info [AccountInfo<'info>])> {
    let split_index = ctx
        .remaining_accounts
        .len()
        .checked_sub(prover_account_count.into())
        .ok_or(Error::from(PortalError::InvalidTokenTransferAccounts))?;

    let (token_transfer_accounts, prover_accounts) = ctx.remaining_accounts.split_at(split_index);

    Ok((token_transfer_accounts.try_into()?, prover_accounts))
}

fn refund_native(ctx: &Context<Refund>, signer_seeds: &[&[u8]]) -> Result<()> {
    match ctx.accounts.vault.lamports() {
        0 => Ok(()),
        amount => invoke_signed(
            &system_instruction::transfer(
                &ctx.accounts.vault.key(),
                &ctx.accounts.creator.key(),
                amount,
            ),
            &[
                ctx.accounts.vault.to_account_info(),
                ctx.accounts.creator.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            &[signer_seeds],
        )
        .map_err(Into::into),
    }
}

fn refund_tokens<'info>(
    ctx: &Context<'info, Refund<'info>>,
    signer_seeds: &[&[u8]],
    accounts: VecTokenTransferAccounts<'info>,
) -> Result<()> {
    accounts
        .into_inner()
        .into_iter()
        .try_for_each(|accounts| refund_token(ctx, signer_seeds, accounts))
}

fn refund_token<'info>(
    ctx: &Context<'info, Refund<'info>>,
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
        token_program.key(),
        CloseAccount {
            account: accounts.from.to_account_info(),
            destination: ctx.accounts.payer.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        },
        &[signer_seeds],
    ))
}
