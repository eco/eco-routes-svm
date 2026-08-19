//! Regression tests for flash-fulfiller's per-prover prove authority and its
//! pinned portal.
//!
//! Security property under test: **a credential flash-fulfiller signs into a
//! caller-chosen program is useless to that program.** The prove leg dispatches
//! to a caller-chosen `local_prover_program`, so its authority is scoped to that
//! program's ID and no honest prover accepts another prover's scoped authority.
//! `portal_program` is pinned outright, since portal is the one program
//! flash-fulfiller can name at compile time.
//!
//! There is deliberately no analogue of `prove_confused_deputy` here. That test
//! works on the portal path because `portal::prove` forwards attacker-supplied
//! `prove_accounts` into the prover, so a malicious prover has the real prover in
//! scope to replay the inherited signer into. `eco_svm_std::prover::prove` — the
//! helper the flash prove leg uses — builds a **fixed six-account** instruction
//! (`caller, payer, system_program, event_authority, callee, proof`) and forwards
//! no tail, so a malicious `local_prover_program` is handed nothing to replay
//! into. Keep it that way: forwarding a tail through that helper would open the
//! path the scoping then has to carry alone.

use flash_fulfiller::instructions::FlashFulfillerError;
use flash_fulfiller::state::{flash_vault_pda, prove_authority_pda};
use portal::types::{Reward, Route};
use solana_sdk::signer::Signer;

pub mod common;

/// Zero-token, zero-native inline intent whose reward names the real
/// local-prover, so the prove leg is the only thing under test.
fn flash_intent(ctx: &mut common::Context) -> (u64, Route, Reward) {
    let now = ctx.now();
    let (destination, mut route, mut reward) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    route.native_amount = 0;
    route.deadline = now + 1800;
    reward.tokens.clear();
    reward.native_amount = 0;
    reward.prover = local_prover::ID;

    (destination, route, reward)
}

#[test]
fn non_portal_program_rejected() {
    let mut ctx = common::Context::default();
    let (_, route, reward) = flash_intent(&mut ctx);
    let claimant = ctx.payer.pubkey();

    let result = ctx.flash_fulfiller().flash_fulfill_via_program(
        &route,
        &reward,
        claimant,
        malicious_prover::ID,
        local_prover::ID,
        prove_authority_pda(&local_prover::ID).0,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidPortalProgram)));
}

/// `prove_authority` must be the PDA scoped to the prover actually being
/// dispatched to. `flash_vault` is the fund-holding identity and was the old
/// (unscoped) prove credential, so it is the specific value that must not work.
#[test]
fn flash_vault_rejected_as_prove_authority() {
    let mut ctx = common::Context::default();
    let (_, route, reward) = flash_intent(&mut ctx);
    let claimant = ctx.payer.pubkey();

    let result = ctx.flash_fulfiller().flash_fulfill_via_program(
        &route,
        &reward,
        claimant,
        portal::ID,
        local_prover::ID,
        flash_vault_pda().0,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidProveAuthority)));
}

/// A `prove_authority` scoped to some *other* prover must not authorize a
/// dispatch to the real local-prover — the scoping is what makes a credential
/// leaked to a malicious prover useless anywhere else.
#[test]
fn other_provers_prove_authority_rejected() {
    let mut ctx = common::Context::default();
    let (_, route, reward) = flash_intent(&mut ctx);
    let claimant = ctx.payer.pubkey();

    let result = ctx.flash_fulfiller().flash_fulfill_via_program(
        &route,
        &reward,
        claimant,
        portal::ID,
        local_prover::ID,
        prove_authority_pda(&malicious_prover::ID).0,
        vec![],
    );

    assert!(result.is_err_and(common::is_error(FlashFulfillerError::InvalidProveAuthority)));
}
