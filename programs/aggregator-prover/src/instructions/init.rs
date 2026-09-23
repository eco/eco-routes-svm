use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;

use crate::instructions::AggregatorProverError;
use crate::state::{Config, CONFIG_SEED, MAX_MEMBERS};

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

pub fn init<'info>(ctx: Context<'info, Init<'info>>, members: Vec<Pubkey>) -> Result<()> {
    require!(
        !members.is_empty() && members.len() <= MAX_MEMBERS,
        AggregatorProverError::InvalidMemberSet
    );
    require!(
        ctx.remaining_accounts.len() == members.len(),
        AggregatorProverError::InvalidMemberSet
    );
    members.iter().enumerate().try_for_each(|(index, member)| {
        require!(
            *member != Pubkey::default()
                && *member != crate::ID
                && ctx.remaining_accounts[index].key() == *member
                && ctx.remaining_accounts[index].executable,
            AggregatorProverError::InvalidMember
        );
        require!(
            !members[..index].contains(member),
            AggregatorProverError::DuplicateMember
        );

        Ok(())
    })?;

    Config { members }.init(
        &ctx.accounts.config,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&[CONFIG_SEED, &[Config::pda().1]]],
    )
}
