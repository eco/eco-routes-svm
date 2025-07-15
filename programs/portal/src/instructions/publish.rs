use anchor_lang::prelude::*;
use tiny_keccak::{Hasher, Keccak};

use crate::events::IntentPublished;
use crate::types::{intent_hash, Reward};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct PublishArgs {
    pub destination: u64,
    pub route: Vec<u8>,
    pub reward: Reward,
}

#[derive(Accounts)]
#[instruction(args: PublishArgs)]
pub struct Publish {}

pub fn publish_intent(_: Context<Publish>, args: PublishArgs) -> Result<()> {
    let PublishArgs {
        destination,
        route,
        reward,
    } = args;

    let mut hasher = Keccak::v256();
    let mut route_hash = [0u8; 32];
    hasher.update(&route);
    hasher.finalize(&mut route_hash);

    let intent_hash = intent_hash(destination, &route_hash.into(), &reward.hash());
    emit!(IntentPublished::new(
        intent_hash,
        destination,
        route,
        reward
    ));

    Ok(())
}
