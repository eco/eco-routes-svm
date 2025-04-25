use anchor_lang::prelude::*;

use crate::{
    error::EcoRoutesError,
    instructions::expected_process_authority_key,
    state::{Intent, IntentStatus},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct HandleFulfilledAckArgs {
    pub intent_hash: [u8; 32],
    pub solver: Pubkey,
}

#[derive(Accounts)]
#[instruction(args: HandleFulfilledAckArgs)]
pub struct HandleFulfilledAck<'info> {
    #[account(
        mut,
        address = expected_process_authority_key() @ EcoRoutesError::InvalidProcessAuthority
    )]
    pub process_authority: Signer<'info>,
    #[account(
        mut,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump = intent.bump,
    )]
    pub intent: Account<'info, Intent>,
}

pub fn handle_fulfilled_ack(
    ctx: Context<HandleFulfilledAck>,
    args: HandleFulfilledAckArgs,
    origin: u32,
    sender: [u8; 32],
) -> Result<()> {
    let intent = &mut ctx.accounts.intent;

    if intent.route.inbox != sender {
        return Err(EcoRoutesError::InvalidSender.into());
    }

    if intent.route.destination_domain_id != origin {
        return Err(EcoRoutesError::InvalidOrigin.into());
    }

    intent.status = IntentStatus::Fulfilled;
    intent.solver = args.solver;

    Ok(())
}
