use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;

use crate::instructions::CctpProverError;
use crate::state::{Config, Peer, CONFIG_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitArgs {
    pub peers: Vec<Peer>,
}

/// One-shot (the config PDA can only be created once) and gated on the
/// program's upgrade authority, so finalizing the program fixes the peer set.
#[derive(Accounts)]
pub struct Init<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ CctpProverError::InvalidAuthority)]
    pub program: Program<'info, crate::program::CctpProver>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ CctpProverError::InvalidAuthority)]
    pub program_data: Account<'info, ProgramData>,
    /// CHECK: canonical PDA, created here
    #[account(mut, address = Config::pda().0 @ CctpProverError::InvalidConfig)]
    pub config: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
    let (_, bump) = Config::pda();

    Config::new(args.peers)?.init(
        &ctx.accounts.config,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&[CONFIG_SEED, &[bump]]],
    )
}
