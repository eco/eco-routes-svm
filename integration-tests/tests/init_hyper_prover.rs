use anchor_lang::error::ErrorCode;
use eco_svm_std::Bytes32;
use hyper_prover::instructions::HyperProverError;
use hyper_prover::state::Config;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

#[test]
fn init_hyper_prover_success() {
    let mut ctx = common::Context::default();
    let sender_1: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let sender_2: Bytes32 = ctx.sender.pubkey().to_bytes().into();
    let whitelisted_senders = vec![sender_1, sender_2];

    let result = ctx
        .hyper_prover()
        .init(whitelisted_senders.clone(), Config::pda().0);
    assert!(result.is_ok());

    let config_pda = Config::pda().0;
    let config: Config = ctx.account(&config_pda).unwrap();
    assert_eq!(config.whitelisted_senders, whitelisted_senders);
    assert!(config.is_whitelisted(&sender_1));
    assert!(config.is_whitelisted(&sender_2));
}

#[test]
fn init_hyper_prover_invalid_config_fail() {
    let mut ctx = common::Context::default();
    let whitelisted_senders = vec![ctx.sender.pubkey().to_bytes().into()];

    let result = ctx
        .hyper_prover()
        .init(whitelisted_senders, Pubkey::new_unique());
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidConfig)));
}

#[test]
fn init_hyper_prover_already_initialized_fail() {
    let mut ctx = common::Context::default();
    let whitelisted_senders = vec![ctx.sender.pubkey().to_bytes().into()];

    ctx.hyper_prover()
        .init(whitelisted_senders.clone(), Config::pda().0)
        .unwrap();

    let result = ctx
        .hyper_prover()
        .init(whitelisted_senders, Config::pda().0);
    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintZero)));
}
