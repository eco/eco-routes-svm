use anchor_lang::prelude::*;
use eco_svm_std::Bytes32;

use crate::events::IntentPublished;
use crate::types::{intent_hash, Intent};

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
        destination,
        route,
        reward,
    } = intent;

    let intent_hash = intent_hash(destination, &route_hash, &reward.hash());
    emit!(IntentPublished::new(intent_hash, route, reward));

    Ok(())
}
