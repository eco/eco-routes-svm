use anchor_lang::prelude::*;

use crate::{error::EcoRoutesError, state::DomainRegistry};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct SetDomainRegistryArgs {
    pub origin_domain_id: u32,
    pub trusted_senders: Vec<[u8; 32]>,
}

#[derive(Accounts)]
#[instruction(args: SetDomainRegistryArgs)]
pub struct SetDomainRegistry<'info> {
    #[account(
        init,
        payer = payer,
        space = DomainRegistry::INIT_SPACE,
        seeds = [b"domain_registry".as_ref(), args.origin_domain_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub domain_registry: Account<'info, DomainRegistry>,

    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: This is the authority
    #[account(address = crate::AUTHORITY @ EcoRoutesError::InvalidAuthority)]
    pub authority: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

pub fn set_domain_registry(
    ctx: Context<SetDomainRegistry>,
    args: SetDomainRegistryArgs,
) -> Result<()> {
    let domain_registry = &mut ctx.accounts.domain_registry;

    domain_registry.validate()?;

    domain_registry.origin_domain_id = args.origin_domain_id;
    domain_registry.trusted_senders = args.trusted_senders;
    domain_registry.bump = ctx.bumps.domain_registry;

    Ok(())
}
