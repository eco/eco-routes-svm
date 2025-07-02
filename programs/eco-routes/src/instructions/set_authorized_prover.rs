use crate::{error::EcoRoutesError, state::EcoRoutes};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct SetAuthorizedProverArgs {
    pub new_prover: [u8; 32],
}

#[derive(Accounts)]
pub struct SetAuthorizedProver<'info> {
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

    pub system_program: Program<'info, System>,
}

pub fn set_authorized_prover(
    ctx: Context<SetAuthorizedProver>,
    args: SetAuthorizedProverArgs,
) -> Result<()> {
    let SetAuthorizedProverArgs { new_prover } = args;
    let eco_routes = &mut ctx.accounts.eco_routes;

    eco_routes.set_authorized_prover(new_prover)?;

    Ok(())
}
