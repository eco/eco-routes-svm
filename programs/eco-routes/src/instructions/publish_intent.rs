use anchor_lang::prelude::*;

use crate::{
    error::EcoRoutesError,
    state::{
        Call, Intent, IntentStatus, Reward, Route, TokenAmount, ValidateCallList,
        ValidateTokenList, MAX_CALLS, MAX_REWARD_TOKENS, MAX_ROUTE_TOKENS,
    },
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct PublishIntentArgs {
    pub salt: [u8; 32],
    pub intent_hash: [u8; 32],
    pub destination_domain_id: u32,
    pub inbox: [u8; 32],
    pub route_tokens: Vec<TokenAmount>,
    pub calls: Vec<Call>,
    pub reward_tokens: Vec<TokenAmount>,
    pub native_reward: u64,
    pub deadline: i64,
    pub calls_root: [u8; 32],
    pub route_root: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: PublishIntentArgs)]
pub struct PublishIntent<'info> {
    #[account(
        init,
        payer = payer,
        space = Intent::INIT_SPACE,
        seeds = [b"intent", args.intent_hash.as_ref()],
        bump,
    )]
    pub intent: Account<'info, Intent>,

    pub creator: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn publish_intent(ctx: Context<PublishIntent>, args: PublishIntentArgs) -> Result<()> {
    let PublishIntentArgs {
        salt,
        intent_hash,
        destination_domain_id,
        inbox,
        route_tokens,
        calls,
        reward_tokens,
        native_reward,
        deadline,
        calls_root,
        route_root,
    } = args;

    let intent = &mut ctx.accounts.intent;
    let creator = &ctx.accounts.creator;

    route_tokens.validate(MAX_ROUTE_TOKENS)?;
    reward_tokens.validate(MAX_REWARD_TOKENS)?;
    calls.validate(MAX_CALLS)?;

    intent.salt = salt;
    intent.intent_hash = intent_hash;
    intent.status = IntentStatus::Initialized;

    intent.creator = creator.key();
    intent.prover = crate::hyperlane::MAILBOX_ID;

    if deadline < Clock::get()?.unix_timestamp {
        return Err(EcoRoutesError::InvalidDeadline.into());
    }

    intent.deadline = deadline;

    intent.route = Route {
        source_domain_id: crate::hyperlane::DOMAIN_ID,
        destination_domain_id,
        inbox,
        prover: crate::hyperlane::MAILBOX_ID,
        tokens: route_tokens,
        tokens_funded: 0,
        calls,
        calls_root,
        route_root,
    };

    intent.reward = Reward {
        tokens: reward_tokens,
        tokens_funded: 0,
        native_reward,
        native_funded: 0,
    };

    intent.bump = ctx.bumps.intent;

    Ok(())
}
