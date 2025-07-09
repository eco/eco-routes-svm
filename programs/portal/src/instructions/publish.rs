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
        destination_chain,
        route,
        reward,
    } = intent;

    // NOTE: We cannot validate that route_hash matches route.hash() because
    // route_hash = hash(destination_chain_encoding(route)), and we cannot implement
    // encoding for all possible destination chains. Each destination chain may have
    // different encoding formats (EVM ABI encoding, Cosmos protobuf, etc.), and
    // implementing all possible encodings would be impractical and tightly couple
    // this Solana program to specific destination chain formats.
    //
    // The route_hash validation is deferred to the fulfill instruction on the
    // destination chain where the complete route data is used to reconstruct the
    // intent_hash for verification using the destination chain's native encoding.
    let intent_hash = intent_hash(destination_chain, &route_hash, &reward.hash());
    emit!(IntentPublished::new(intent_hash, route, reward));

    Ok(())
}
