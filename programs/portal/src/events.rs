use anchor_lang::prelude::*;
use derive_new::new;

use crate::types::{Bytes32, Reward, Route};

#[event]
#[derive(new)]
pub struct IntentPublished {
    intent_hash: Bytes32,
    route: Route,
    reward: Reward,
}
