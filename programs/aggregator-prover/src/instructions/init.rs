use std::collections::HashSet;

use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;

use crate::instructions::AggregatorProverError;
use crate::state::{Config, CONFIG_SEED, MAX_PROVERS};

#[derive(Accounts)]
pub struct Init<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ AggregatorProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::AggregatorProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ AggregatorProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    /// CHECK: canonical PDA, initialized once with AccountExt.
    #[account(mut, address = Config::pda().0 @ AggregatorProverError::InvalidConfig)]
    pub config: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn init<'info>(ctx: Context<'info, Init<'info>>) -> Result<()> {
    let provers = ctx.remaining_accounts;
    require!(
        !provers.is_empty() && provers.len() <= MAX_PROVERS,
        AggregatorProverError::InvalidProverSet
    );

    let mut seen = HashSet::with_capacity(provers.len());
    provers.iter().try_for_each(|prover| {
        require!(
            prover.key() != Pubkey::default() && prover.key() != crate::ID && prover.executable,
            AggregatorProverError::InvalidProver
        );
        require!(
            seen.insert(prover.key()),
            AggregatorProverError::DuplicateProver
        );

        Ok(())
    })?;

    let provers = provers.iter().map(|prover| prover.key()).collect();

    Config { provers }.init(
        &ctx.accounts.config,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&[CONFIG_SEED, &[Config::pda().1]]],
    )
}
