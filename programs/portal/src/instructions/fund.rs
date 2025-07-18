use std::collections::BTreeMap;

use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::{associated_token, token, token_2022};
use eco_svm_std::Bytes32;

use crate::events::IntentFunded;
use crate::instructions::fund_context::FundTokenContext;
use crate::instructions::PortalError;
use crate::state::vault_pda;
use crate::types::{self, Reward, VecTokenTransferAccounts};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct FundArgs {
    pub destination: u64,
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
    #[account(mut)]
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
        destination,
        route_hash,
        reward,
        allow_partial,
    } = args;
    let intent_hash = types::intent_hash(destination, &route_hash, &reward.hash());

    require!(
        ctx.accounts.vault.key() == vault_pda(&intent_hash).0,
        PortalError::InvalidVault
    );

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
                intent_hash,
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
    accounts: VecTokenTransferAccounts<'info>,
    reward_token_amounts: &BTreeMap<Pubkey, u64>,
) -> Result<usize> {
    FundTokenContext::from(ctx)
        .fund_tokens(accounts, reward_token_amounts)
        .map(|funded_tokens| funded_tokens.len())
}
