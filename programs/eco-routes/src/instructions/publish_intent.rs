use anchor_lang::prelude::*;

use crate::state::{Call, Intent, Reward, Route, TokenAmount};

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

    *ctx.accounts.intent = Intent::new(
        intent_hash,
        Route::new(salt, destination_domain_id, inbox, route_tokens, calls)?,
        Reward::new(
            reward_tokens,
            ctx.accounts.creator.key(),
            native_reward,
            deadline,
        )?,
        ctx.bumps.intent,
    )?;

    Ok(())
}
