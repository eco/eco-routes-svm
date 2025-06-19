use crate::{error::EcoRoutesError, state::EcoRoutes};
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct SetAuthority<'info> {
    #[account(
        mut,
        seeds = [b"eco_routes"],
        bump
    )]
    pub eco_routes: Account<'info, EcoRoutes>,

    #[account(
        mut,
        address = eco_routes.authority @ EcoRoutesError::InvalidAuthority
    )]
    pub authority: Signer<'info>,

    /// CHECK: no checkes needed to set it as a new authority
    #[account()]
    pub new_authority: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn set_authority(ctx: Context<SetAuthority>) -> Result<()> {
    let new_authority_pubkey = *ctx.accounts.new_authority.key;
    let eco_routes = &mut ctx.accounts.eco_routes;

    eco_routes.set_authority(new_authority_pubkey)?;

    Ok(())
}
