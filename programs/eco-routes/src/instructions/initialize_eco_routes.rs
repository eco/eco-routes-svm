use crate::{
    error::EcoRoutesError,
    state::{EcoRoutes, ECO_ROUTES_AUTHORITY},
};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct InitializeEcoRoutesArgs {
    pub prover: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: InitializeEcoRoutesArgs)]
pub struct InitializeEcoRoutes<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + EcoRoutes::INIT_SPACE,
        seeds = [b"eco_routes"],
        bump
    )]
    pub eco_routes: Account<'info, EcoRoutes>,

    #[account(
        mut,
        address = ECO_ROUTES_AUTHORITY.parse::<Pubkey>().unwrap() @ EcoRoutesError::InvalidAuthority
    )]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn initialize_eco_routes(
    ctx: Context<InitializeEcoRoutes>,
    args: InitializeEcoRoutesArgs,
) -> Result<()> {
    let InitializeEcoRoutesArgs { prover } = args;
    let authority_pubkey = ctx.accounts.authority.key;

    *ctx.accounts.eco_routes = EcoRoutes::new(*authority_pubkey, prover, ctx.bumps.eco_routes);

    Ok(())
}
