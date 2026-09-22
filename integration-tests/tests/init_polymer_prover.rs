use anchor_lang::error::ErrorCode;
use anchor_lang::Space;
use eco_svm_std::Bytes32;
use polymer_prover::instructions::PolymerProverError;
use polymer_prover::state::{Config, MAX_WHITELIST_LEN};
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

/// An empty whitelist is accepted and permanent: there is no setter and the PDA
/// can only be created once, so a prover initialised this way can never validate
/// a proof. Deliberate — spec section 4 accepts config immutability with an
/// unauthenticated `init`, and Rollout step 4 gates it by reading `Config` back
/// after `init` and burning the program ID on mismatch. The other end of the
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

/// `init` is one-shot, unauthenticated and burn-on-failure (Rollout step 4), so
/// the whitelist cap is a path with no retry. Two things are only reachable at
/// the cap: `AccountExt::init` allocates `8 + Config::INIT_SPACE` and
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
