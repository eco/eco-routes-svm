use std::collections::{BTreeMap, BTreeSet};

use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token_interface::TokenAccount;
use anchor_spl::{associated_token, token, token_2022};
use eco_svm_std::Bytes32;

use crate::events::IntentFunded;
use crate::instructions::PortalError;
use crate::state;
use crate::types::{self, Reward, TokenTransferAccounts, VecTokenTransferAccounts};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct FundArgs {
    pub destination_chain: Bytes32,
    pub route_hash: Bytes32,
    pub reward: Reward,
    pub allow_partial: bool,
}

#[derive(Accounts)]
#[instruction(args: FundArgs)]
pub struct Fund<'info> {
    pub payer: Signer<'info>,
    #[account(mut)]
    pub funder: Signer<'info>,
    /// CHECK: address is validated
    #[account(mut, address = state::vault_pda(&types::intent_hash(&args.destination_chain, &args.route_hash, &args.reward)).0 @ PortalError::InvalidVault)]
    pub vault: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub associated_token_program: Program<'info, associated_token::AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn fund_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, Fund<'info>>,
    args: FundArgs,
) -> Result<()> {
    let FundArgs {
        destination_chain,
        route_hash,
        reward,
        allow_partial,
    } = args;

    let native_funded = fund_vault_native(&ctx, &reward)?;

    let reward_token_amounts = reward.token_amounts()?;
    let token_funded_count = fund_vault_tokens(
        &ctx,
        ctx.remaining_accounts.try_into()?,
        &reward_token_amounts,
    )?;

    let funded_count = native_funded as usize + token_funded_count;

    match (allow_partial, funded_count) {
        (false, funded_count) if funded_count < reward_token_amounts.len() + 1 => {
            Err(PortalError::InsufficientFunds.into())
        }
        (_, funded_count) => {
            emit!(IntentFunded::new(
                types::intent_hash(&destination_chain, &route_hash, &reward),
                ctx.accounts.funder.key(),
                funded_count == reward_token_amounts.len() + 1,
            ));

            Ok(())
        }
    }
}

fn fund_vault_native<'info>(
    ctx: &Context<'_, '_, '_, 'info, Fund<'info>>,
    reward: &Reward,
) -> Result<bool> {
    reward
        .native_amount
        .checked_sub(ctx.accounts.vault.lamports())
        .map(|amount| amount.min(ctx.accounts.funder.lamports()))
        .filter(|&amount| amount > 0)
        .map(|amount| {
            system_program::transfer(
                CpiContext::new(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.funder.to_account_info(),
                        to: ctx.accounts.vault.to_account_info(),
                    },
                ),
                amount,
            )
        })
        .transpose()
        .map(|_| ctx.accounts.vault.lamports() >= reward.native_amount)
}

fn fund_vault_tokens<'info>(
    ctx: &Context<'_, '_, '_, 'info, Fund<'info>>,
    fund_token_accounts: VecTokenTransferAccounts<'info>,
    reward_token_amounts: &BTreeMap<Pubkey, u64>,
) -> Result<usize> {
    let funded_token = fund_token_accounts
        .into_inner()
        .into_iter()
        .map(|fund_token_accounts| fund_vault_token(ctx, fund_token_accounts, reward_token_amounts))
        .filter_map(|result| match result {
            Ok(Some(mint_key)) => Some(Ok(mint_key)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<BTreeSet<_>>>()?;

    Ok(funded_token.len())
}

fn fund_vault_token<'info>(
    ctx: &Context<'_, '_, '_, 'info, Fund<'info>>,
    accounts: TokenTransferAccounts<'info>,
    reward_token_amounts: &BTreeMap<Pubkey, u64>,
) -> Result<Option<Pubkey>> {
    let mint_key = accounts.mint.key();
    let vault_ata = get_associated_token_address_with_program_id(
        ctx.accounts.vault.key,
        &mint_key,
        accounts.token_program_id(),
    );
    require!(vault_ata == accounts.to.key(), PortalError::InvalidVaultAta);

    let token_program = accounts.token_program(
        &ctx.accounts.token_program,
        &ctx.accounts.token_2022_program,
    )?;
    let reward_token_amount = reward_token_amounts
        .get(&mint_key)
        .ok_or(PortalError::InvalidMint)?;
    let to_data = ensure_initialized(ctx, &accounts.mint, &accounts.to, &token_program)?;
    let from_data = accounts.from_data()?;

    reward_token_amount
        .checked_sub(to_data.amount)
        .map(|amount| amount.min(from_data.amount))
        .filter(|&amount| amount > 0)
        .map(|amount| accounts.transfer(&token_program, &ctx.accounts.funder, amount))
        .transpose()?;

    if accounts.to_data()?.amount >= *reward_token_amount {
        Ok(Some(mint_key))
    } else {
        Ok(None)
    }
}

fn ensure_initialized<'info>(
    ctx: &Context<'_, '_, '_, 'info, Fund<'info>>,
    mint: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
) -> Result<TokenAccount> {
    if to.data_is_empty() {
        let cpi_accounts = associated_token::Create {
            payer: ctx.accounts.payer.to_account_info(),
            associated_token: to.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
            mint: mint.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            token_program: token_program.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(
            ctx.accounts.associated_token_program.to_account_info(),
            cpi_accounts,
        );

        associated_token::create(cpi_ctx)?;
    }

    TokenAccount::try_deserialize(&mut &to.try_borrow_data()?[..])
}
