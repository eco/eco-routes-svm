use anchor_lang::error::ErrorCode;
use anchor_lang::{InstructionData, Space, ToAccountMetas};
use eco_svm_std::Bytes32;
use polymer_prover::instructions::PolymerProverError;
use polymer_prover::state::{Config, MAX_WHITELIST_LEN};
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

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

#[test]
fn init_polymer_prover_wrong_authority_fail() {
    let mut ctx = common::Context::default();

    let result = ctx.polymer_prover().init_with_authority(
        &Keypair::new(),
        vec![emitter(1)],
        Config::pda().0,
    );
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidAuthority)));
    assert!(ctx.get_account(&Config::pda().0).is_none());
}

#[test]
fn init_polymer_prover_immutable_program_fail() {
    let mut ctx = common::Context::default();
    ctx.set_upgrade_authority(&polymer_prover::ID, None);

    let result = ctx.polymer_prover().init(vec![emitter(1)], Config::pda().0);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidAuthority)));
    assert!(ctx.get_account(&Config::pda().0).is_none());
}

/// An empty whitelist is accepted and permanent: there is no setter and the PDA
/// can only be created once, so a prover initialised this way can never validate
/// a proof. Deliberate — spec section 4 accepts config immutability, and Rollout
/// step 4 gates it by reading `Config` back after `init` and burning the program
/// ID on mismatch. The other end of the
/// range is `init_polymer_prover_at_max_whitelist_success`;
/// `TooManyWhitelistedEmitters` itself is pinned by the `Config::new` unit tests.
#[test]
fn init_polymer_prover_empty_whitelist_accepts_nothing() {
    let mut ctx = common::Context::default();
    ctx.polymer_prover().init(vec![], Config::pda().0).unwrap();

    let config: Config = ctx.account(&Config::pda().0).unwrap();
    assert!(config.whitelisted_emitters.is_empty());
    assert!(!config.is_whitelisted(&emitter(1)));
}

/// Two things are only reachable at the whitelist cap: `AccountExt::init` allocates `8 + Config::INIT_SPACE` and
/// `try_serialize`s into that exact slice, so a `#[max_len]` that drifts below
/// `MAX_WHITELIST_LEN` fails here and nowhere else; and the 652 bytes of
/// instruction data must still fit one legacy packet (~915 bytes against 1232),
/// since no relayer-side lookup table exists at deploy time.
#[test]
fn init_polymer_prover_at_max_whitelist_success() {
    let mut ctx = common::Context::default();
    let emitters: Vec<_> = (0..MAX_WHITELIST_LEN)
        .map(|i| emitter(u8::try_from(i + 1).unwrap()))
        .collect();

    ctx.polymer_prover()
        .init(emitters.clone(), Config::pda().0)
        .unwrap();

    // The raw length, not just a successful decode: `ctx.account::<Config>`
    // alone would not catch an over-allocation.
    let raw = ctx.get_account(&Config::pda().0).unwrap();
    assert_eq!(raw.data.len(), 8 + Config::INIT_SPACE);

    let config: Config = ctx.account(&Config::pda().0).unwrap();
    assert_eq!(config.whitelisted_emitters, emitters);
    assert!(emitters.iter().all(|e| config.is_whitelisted(e)));
}

/// A caller cannot vouch for itself with another program's program data.
#[test]
fn init_polymer_prover_foreign_program_data_fail() {
    let mut ctx = common::Context::default();
    let attacker = Keypair::new();
    ctx.set_upgrade_authority(&local_prover::ID, Some(attacker.pubkey()));
    let loader = ctx.get_account(&polymer_prover::ID).unwrap().owner;
    let instruction = Instruction {
        program_id: polymer_prover::ID,
        accounts: polymer_prover::accounts::Init {
            config: Config::pda().0,
            payer: ctx.payer.pubkey(),
            authority: attacker.pubkey(),
            program: polymer_prover::ID,
            program_data: Pubkey::find_program_address(&[local_prover::ID.as_ref()], &loader).0,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None),
        data: polymer_prover::instruction::Init {
            args: polymer_prover::instructions::InitArgs {
                whitelisted_emitters: vec![emitter(1)],
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
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidAuthority)));
    assert!(ctx.get_account(&Config::pda().0).is_none());
}
