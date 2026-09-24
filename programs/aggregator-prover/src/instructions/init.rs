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

pub fn init<'info>(ctx: Context<'info, Init<'info>>, provers: Vec<Pubkey>) -> Result<()> {
    require!(
        !provers.is_empty() && provers.len() <= MAX_PROVERS,
        AggregatorProverError::InvalidProverSet
    );
    require!(
        ctx.remaining_accounts.len() == provers.len(),
        AggregatorProverError::InvalidProverSet
    );
    provers.iter().enumerate().try_for_each(|(index, prover)| {
        require!(
            *prover != Pubkey::default()
                && *prover != crate::ID
                && ctx.remaining_accounts[index].key() == *prover
                && ctx.remaining_accounts[index].executable,
            AggregatorProverError::InvalidProver
        );
        require!(
            !provers[..index].contains(prover),
            AggregatorProverError::DuplicateProver
        );

        Ok(())
    })?;

    Config { provers }.init(
        &ctx.accounts.config,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&[CONFIG_SEED, &[Config::pda().1]]],
    )
}
