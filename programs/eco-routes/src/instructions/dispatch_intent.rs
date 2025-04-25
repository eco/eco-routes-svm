use anchor_lang::{
    prelude::*,
    solana_program::{instruction::Instruction, program::invoke_signed},
};

use crate::{
    error::EcoRoutesError,
    hyperlane,
    state::{Intent, IntentStatus},
};

use super::{HandleBlueprintArgs, InboxPayload};

#[derive(Accounts)]
pub struct DispatchIntent<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: just a signer
    #[account(
        seeds = [b"hyperlane", b"-", b"dispatch_authority"],
        bump,
        address = dispatch_authority_key().0 @ EcoRoutesError::InvalidDispatchAuthority
    )]
    pub dispatch_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [b"intent", intent.intent_hash.as_ref()],
        bump = intent.bump,
        constraint = intent.status == IntentStatus::Funded @ EcoRoutesError::IntentNotFunded,
    )]
    pub intent: Account<'info, Intent>,

    /// CHECK: Address is enforced
    #[account(address = hyperlane::MAILBOX_ID @ EcoRoutesError::NotMailbox)]
    pub mailbox_program: UncheckedAccount<'info>,

    /// CHECK: Writable Outbox PDA belonging to the Mailbox
    #[account(mut)]
    pub outbox_pda: UncheckedAccount<'info>,

    /// System program
    pub system_program: Program<'info, System>,

    /// CHECK: SPL Noop program (Noop111111111111111111111111111111111111111)
    pub spl_noop_program: UncheckedAccount<'info>,

    #[account(mut)]
    pub unique_message: Signer<'info>,

    /// CHECK: Writable PDA derived from `unique_message`
    #[account(mut)]
    pub dispatched_message_pda: UncheckedAccount<'info>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
struct OutboxDispatch {
    sender: Pubkey,
    destination_domain: u32,
    recipient: [u8; 32],
    message_body: Vec<u8>,
}

const OUTBOX_DISPATCH_VARIANT: u8 = 4;

pub fn dispatch_intent(ctx: Context<DispatchIntent>) -> Result<()> {
    let intent = &mut ctx.accounts.intent;

    require!(
        intent.status == IntentStatus::Funded,
        EcoRoutesError::IntentNotFunded
    );
    require!(!intent.is_expired()?, EcoRoutesError::DeadlinePassed);

    let blueprint = HandleBlueprintArgs {
        salt: intent.salt,
        intent_hash: intent.intent_hash,
        route_root: intent.route.route_root,
        calls_root: intent.route.calls_root,
        deadline: intent.deadline,
    };
    let payload = InboxPayload::Blueprint(blueprint)
        .try_to_vec()
        .map_err(|_| error!(EcoRoutesError::InvalidHandlePayload))?;

    let outbox_dispatch = OutboxDispatch {
        sender: ctx.accounts.dispatch_authority.key(),
        destination_domain: intent.route.destination_domain_id,
        recipient: intent.route.inbox,
        message_body: payload,
    };
    let mut ix_data = Vec::new();
    ix_data.push(OUTBOX_DISPATCH_VARIANT);
    ix_data.extend(outbox_dispatch.try_to_vec()?);

    let metas = vec![
        AccountMeta::new(ctx.accounts.outbox_pda.key(), false),
        AccountMeta::new_readonly(ctx.accounts.dispatch_authority.key(), true),
        AccountMeta::new_readonly(ctx.accounts.system_program.key(), false),
        AccountMeta::new_readonly(ctx.accounts.spl_noop_program.key(), false),
        AccountMeta::new_readonly(ctx.accounts.payer.key(), true),
        AccountMeta::new_readonly(ctx.accounts.unique_message.key(), true),
        AccountMeta::new(ctx.accounts.dispatched_message_pda.key(), false),
    ];

    let ix = Instruction {
        program_id: ctx.accounts.mailbox_program.key(),
        accounts: metas,
        data: ix_data,
    };

    invoke_signed(
        &ix,
        &[
            ctx.accounts.outbox_pda.to_account_info(),
            ctx.accounts.dispatch_authority.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.spl_noop_program.to_account_info(),
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.unique_message.to_account_info(),
            ctx.accounts.dispatched_message_pda.to_account_info(),
        ],
        &[&[
            b"hyperlane",
            b"-",
            b"dispatch_authority",
            &[ctx.bumps.dispatch_authority],
        ]],
    )?;

    intent.status = IntentStatus::Dispatched;
    Ok(())
}

pub fn dispatch_authority_key() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"hyperlane", b"-", b"dispatch_authority"], &crate::ID)
}
