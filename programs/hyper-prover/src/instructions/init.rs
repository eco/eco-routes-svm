use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::Bytes32;

use crate::instructions::HyperProverError;
use crate::state::{Config, CONFIG_SEED};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitArgs {
    pub whitelisted_senders: Vec<Bytes32>,
}

#[derive(Accounts)]
#[instruction(args: InitArgs)]
pub struct Init<'info> {
    /// CHECK: address is validated
    #[account(mut)]
    pub config: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

pub fn init(ctx: Context<Init>, args: InitArgs) -> Result<()> {
    let (config_pda, bump) = Config::pda();
    require!(
        ctx.accounts.config.key() == config_pda,
        HyperProverError::InvalidConfig
    );
    let signer_seeds = [CONFIG_SEED, &[bump]];

    Config::new(args.whitelisted_senders)?.init(
        &ctx.accounts.config,
        &ctx.accounts.payer,
        &ctx.accounts.system_program,
        &[&signer_seeds],
    )
}
