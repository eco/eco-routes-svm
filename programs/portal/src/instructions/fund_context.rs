use std::collections::{BTreeMap, BTreeSet};

use anchor_lang::prelude::*;
use anchor_spl::associated_token::{self, get_associated_token_address_with_program_id};
use anchor_spl::token_interface::TokenAccount;
use anchor_spl::{token, token_2022};

use crate::instructions::{Fulfill, Fund, PortalError};
use crate::types::{TokenTransferAccounts, VecTokenTransferAccounts};

pub struct FundTokenContext<'a, 'info> {
    pub payer: &'a Signer<'info>,
    pub funder: &'a Signer<'info>,
    pub fundee: AccountInfo<'info>,
    pub token_program: &'a Program<'info, token::Token>,
    pub token_2022_program: &'a Program<'info, token_2022::Token2022>,
    pub associated_token_program: &'a Program<'info, associated_token::AssociatedToken>,
    pub system_program: &'a Program<'info, System>,
}

impl<'a, 'info> From<&'a Context<'_, '_, '_, 'info, Fund<'info>>> for FundTokenContext<'a, 'info> {
    fn from(ctx: &'a Context<'_, '_, '_, 'info, Fund<'info>>) -> Self {
        Self {
            payer: &ctx.accounts.payer,
            funder: &ctx.accounts.funder,
            fundee: ctx.accounts.vault.to_account_info(),
            token_program: &ctx.accounts.token_program,
            token_2022_program: &ctx.accounts.token_2022_program,
            associated_token_program: &ctx.accounts.associated_token_program,
            system_program: &ctx.accounts.system_program,
        }
    }
}

impl<'a, 'info> From<&'a Context<'_, '_, '_, 'info, Fulfill<'info>>>
    for FundTokenContext<'a, 'info>
{
    fn from(ctx: &'a Context<'_, '_, '_, 'info, Fulfill<'info>>) -> Self {
        Self {
            payer: &ctx.accounts.payer,
            funder: &ctx.accounts.solver,
            fundee: ctx.accounts.executor.to_account_info(),
            token_program: &ctx.accounts.token_program,
            token_2022_program: &ctx.accounts.token_2022_program,
            associated_token_program: &ctx.accounts.associated_token_program,
            system_program: &ctx.accounts.system_program,
        }
    }
}

impl<'info> FundTokenContext<'_, 'info> {
    pub fn fund_tokens(
        self,
        accounts: VecTokenTransferAccounts<'info>,
        token_amounts: &BTreeMap<Pubkey, u64>,
    ) -> Result<BTreeSet<Pubkey>> {
        accounts
            .into_inner()
            .into_iter()
            .map(|accounts| self.fund_token(accounts, token_amounts))
            .filter_map(|result| match result {
                Ok(Some(mint_key)) => Some(Ok(mint_key)),
                Ok(None) => None,
                Err(e) => Some(Err(e)),
            })
            .collect()
    }

    fn fund_token(
        &self,
        accounts: TokenTransferAccounts<'info>,
        token_amounts: &BTreeMap<Pubkey, u64>,
    ) -> Result<Option<Pubkey>> {
        let token_program = accounts.token_program(self.token_program, self.token_2022_program)?;
        let token_amount = token_amounts
            .get(accounts.mint.key)
            .ok_or(PortalError::InvalidMint)?;
        let to_data = self.ensure_fundee_ata_initialized(
            &accounts.mint,
            &accounts.to,
            &token_program,
            accounts.token_program_id(),
        )?;
        let from_data = accounts.from_data()?;

        token_amount
            .checked_sub(to_data.amount)
            .map(|amount| amount.min(from_data.amount))
            .filter(|&amount| amount > 0)
            .map(|amount| accounts.transfer(&token_program, self.funder, amount))
            .transpose()?;

        if accounts.to_data()?.amount >= *token_amount {
            Ok(Some(accounts.mint.key()))
        } else {
            Ok(None)
        }
    }

    fn ensure_fundee_ata_initialized(
        &self,
        mint: &AccountInfo<'info>,
        to: &AccountInfo<'info>,
        token_program: &AccountInfo<'info>,
        program_id: &Pubkey,
    ) -> Result<TokenAccount> {
        let fundee_ata =
            get_associated_token_address_with_program_id(self.fundee.key, mint.key, program_id);
        require!(fundee_ata == *to.key, PortalError::InvalidAta);

        if to.data_is_empty() {
            let cpi_accounts = associated_token::Create {
                payer: self.payer.to_account_info(),
                associated_token: to.to_account_info(),
                authority: self.fundee.to_account_info(),
                mint: mint.to_account_info(),
                system_program: self.system_program.to_account_info(),
                token_program: token_program.to_account_info(),
            };
            let cpi_ctx = CpiContext::new(
                self.associated_token_program.to_account_info(),
                cpi_accounts,
            );

            associated_token::create(cpi_ctx)?;
        }

        TokenAccount::try_deserialize(&mut &to.try_borrow_data()?[..])
    }
}
