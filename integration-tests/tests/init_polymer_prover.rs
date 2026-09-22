use anchor_lang::error::ErrorCode;
use eco_svm_std::Bytes32;
use polymer_prover::instructions::PolymerProverError;
use polymer_prover::state::Config;
use solana_sdk::pubkey::Pubkey;

pub mod common;

fn emitter(byte: u8) -> Bytes32 {
    polymer_prover::event::evm_address_to_bytes32([byte; 20])
}

#[test]
fn init_polymer_prover_success() {
    let mut ctx = common::Context::default();
    let emitters = vec![emitter(1), emitter(2)];

    let result = ctx.polymer_prover().init(emitters.clone(), Config::pda().0);
    assert!(result.is_ok());

    let config: Config = ctx.account(&Config::pda().0).unwrap();
    assert_eq!(config.whitelisted_emitters, emitters);
    assert!(config.is_whitelisted(&emitter(1)));
    assert!(!config.is_whitelisted(&emitter(3)));
}

#[test]
fn init_polymer_prover_invalid_config_fail() {
    let mut ctx = common::Context::default();

    let result = ctx
        .polymer_prover()
        .init(vec![emitter(1)], Pubkey::new_unique());
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidConfig)));
}

#[test]
fn init_polymer_prover_already_initialized_fail() {
    let mut ctx = common::Context::default();
    ctx.polymer_prover()
        .init(vec![emitter(1)], Config::pda().0)
        .unwrap();

    let result = ctx.polymer_prover().init(vec![emitter(1)], Config::pda().0);
    assert!(result.is_err_and(common::is_error(ErrorCode::ConstraintZero)));
}
