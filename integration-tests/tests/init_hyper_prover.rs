use anchor_lang::error::ErrorCode;
use anchor_lang::{InstructionData, ToAccountMetas};
use eco_svm_std::Bytes32;
use hyper_prover::instructions::HyperProverError;
use hyper_prover::state::Config;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

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

#[test]
fn init_hyper_prover_wrong_authority_fail() {
    let mut ctx = common::Context::default();
    let whitelisted_senders = vec![ctx.sender.pubkey().to_bytes().into()];

    let result = ctx.hyper_prover().init_with_authority(
        &Keypair::new(),
        whitelisted_senders,
        Config::pda().0,
    );
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidAuthority)));
    assert!(ctx.get_account(&Config::pda().0).is_none());
}

#[test]
fn init_hyper_prover_immutable_program_fail() {
    let mut ctx = common::Context::default();
    let whitelisted_senders = vec![ctx.sender.pubkey().to_bytes().into()];
    ctx.set_upgrade_authority(&hyper_prover::ID, None);

    let result = ctx
        .hyper_prover()
        .init(whitelisted_senders, Config::pda().0);
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidAuthority)));
    assert!(ctx.get_account(&Config::pda().0).is_none());
}

/// A caller cannot vouch for itself with another program's program data.
#[test]
fn init_hyper_prover_foreign_program_data_fail() {
    let mut ctx = common::Context::default();
    let attacker = Keypair::new();
    ctx.set_upgrade_authority(&local_prover::ID, Some(attacker.pubkey()));
    let loader = ctx.get_account(&hyper_prover::ID).unwrap().owner;
    let instruction = Instruction {
        program_id: hyper_prover::ID,
        accounts: hyper_prover::accounts::Init {
            config: Config::pda().0,
            payer: ctx.payer.pubkey(),
            authority: attacker.pubkey(),
            program: hyper_prover::ID,
            program_data: Pubkey::find_program_address(&[local_prover::ID.as_ref()], &loader).0,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: hyper_prover::instruction::Init {
            args: hyper_prover::instructions::InitArgs {
                whitelisted_senders: vec![ctx.sender.pubkey().to_bytes().into()],
            },
        }
        .data(),
    };
    let transaction = Transaction::new(
        &[&ctx.payer, &attacker],
        Message::new(&[instruction], Some(&ctx.payer.pubkey())),
        ctx.latest_blockhash(),
    );

    let result = ctx.send_transaction(transaction);
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidAuthority)));
    assert!(ctx.get_account(&Config::pda().0).is_none());
}
