use anchor_lang::prelude::*;

use crate::events::IntentPublished;
use crate::types::{intent_hash, Bytes32, Intent};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct PublishArgs {
    pub intent: Intent,
    pub route_hash: Bytes32,
}

#[derive(Accounts)]
#[instruction(args: PublishArgs)]
pub struct Publish {}

pub fn publish_intent(_: Context<Publish>, args: PublishArgs) -> Result<()> {
    let PublishArgs { intent, route_hash } = args;
    let Intent {
        route_chain,
        route,
        reward,
    } = intent;

    let intent_hash = intent_hash(route_chain, route_hash, &reward);
    emit!(IntentPublished::new(intent_hash, route, reward));

    Ok(())
}
