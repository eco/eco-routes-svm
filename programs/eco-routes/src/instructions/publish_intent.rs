use anchor_lang::prelude::*;

use crate::{
    encoding, encoding_two,
    error::EcoRoutesError,
    instructions::expected_prover_process_authority,
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
}

#[derive(Accounts)]
#[instruction(args: PublishIntentArgs)]
pub struct PublishIntent<'info> {
    #[account(
        init,
        payer = payer,
        space = 8 + Intent::INIT_SPACE,
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
    } = args;

    let intent = &mut ctx.accounts.intent;
    let creator = &ctx.accounts.creator;

    route_tokens.validate(MAX_ROUTE_TOKENS)?;
    reward_tokens.validate(MAX_REWARD_TOKENS)?;
    calls.validate(MAX_CALLS)?;

    intent.intent_hash = intent_hash;
    intent.status = IntentStatus::Initialized;

    if deadline < Clock::get()?.unix_timestamp {
        return Err(EcoRoutesError::InvalidDeadline.into());
    }

    intent.route = Route {
        salt,
        source_domain_id: crate::hyperlane::DOMAIN_ID,
        destination_domain_id,
        inbox,
        tokens: route_tokens,
        calls,
    };

    intent.reward = Reward {
        creator: creator.key(),
        prover: crate::hyperlane::MAILBOX_ID.to_bytes(),
        tokens: reward_tokens,
        native_amount: native_reward,
        deadline,
    };

    intent.bump = ctx.bumps.intent;

    let expected_intent_hash = encoding_two::get_intent_hash(&intent.route, &intent.reward);
    require!(
        intent_hash == expected_intent_hash,
        EcoRoutesError::InvalidIntentHash
    );

    Ok(())
}
