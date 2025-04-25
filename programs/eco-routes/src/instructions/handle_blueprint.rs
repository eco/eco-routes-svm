use anchor_lang::prelude::*;

use crate::{
    error::EcoRoutesError,
    instructions::expected_process_authority_key,
    state::{DomainRegistry, IntentMarker},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct HandleBlueprintArgs {
    pub salt: [u8; 32],
    pub intent_hash: [u8; 32],
    pub route_root: [u8; 32],
    pub calls_root: [u8; 32],
    pub deadline: i64,
}

#[derive(Accounts)]
#[instruction(args: HandleBlueprintArgs, origin: u32)]
pub struct HandleBlueprint<'info> {
    #[account(
        mut,
        address = expected_process_authority_key() @ EcoRoutesError::InvalidProcessAuthority
    )]
    pub process_authority: Signer<'info>,
    #[account(
        init,
        payer = process_authority,
        space = IntentMarker::INIT_SPACE,
        seeds = [b"intent_marker".as_ref(), args.intent_hash.as_ref()],
        bump,
    )]
    pub intent_marker: Account<'info, IntentMarker>,
    #[account(
        seeds = [b"domain_registry".as_ref(), origin.to_le_bytes().as_ref()],
        bump = domain_registry.bump,
    )]
    pub domain_registry: Account<'info, DomainRegistry>,
    pub system_program: Program<'info, System>,
}

pub fn handle_blueprint(
    ctx: Context<HandleBlueprint>,
    args: HandleBlueprintArgs,
    origin: u32,
    sender: [u8; 32],
) -> Result<()> {
    let intent_marker = &mut ctx.accounts.intent_marker;
    let domain_registry = &ctx.accounts.domain_registry;

    if !domain_registry.is_sender_trusted(origin, &sender) {
        return Err(EcoRoutesError::InvalidSender.into());
    }

    intent_marker.bump = ctx.bumps.intent_marker;
    intent_marker.source_domain_id = origin;
    intent_marker.intent_hash = args.intent_hash;
    intent_marker.calls_root = args.calls_root;
    intent_marker.route_root = args.route_root;
    intent_marker.deadline = args.deadline;

    Ok(())
}
