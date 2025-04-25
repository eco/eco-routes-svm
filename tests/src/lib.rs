#![cfg(test)]
pub mod multisig_ism_stub;
pub mod utils;

pub mod common;

pub mod acknowledge_fulfillment;
pub mod claim_intent;
pub mod create_intent;
pub mod dispatch_intent;
pub mod fund_intent;
pub mod receive_blueprint;
pub mod refund_intent;

#[test]
pub fn test() -> anyhow::Result<()> {
    Ok(())
}
