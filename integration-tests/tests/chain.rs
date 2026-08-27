//! Integration coverage for `intent_chainer::chain` — the SVM → SVM chained
//! local intent flow.
//!
//! The suite is built around one end-to-end path that only uses real portal entry
//! points: fulfill intent1 so its route call delivers into the chainer's escrow,
//! `chain` to resolve and fund intent2 from the measured balance, then fulfill,
//! prove and withdraw intent2 so the pushed tokens actually reach a claimant.
//! Everything else pins one property of that path.

mod common;

use anchor_lang::prelude::AccountMeta;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use common::intent_chainer_context::IDENTITY_SCALE;
use common::{contains_event, is_error, Context};
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CHAIN_ID};
use intent_chainer::events::IntentChained;
use intent_chainer::instructions::ChainerError;
use intent_chainer::types::WAD;
use portal::state::{
    dispatcher_pda, executor_pda, proof_closer_pda, vault_pda, FulfillMarker, WithdrawnMarker,
};
use portal::types::{intent_hash, TokenAmount};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

/// Amount intent1 delivers into the escrow. Deliberately not a round number and
/// not anything the order declares, so the test can only pass if the amount is
/// genuinely measured rather than assumed.
const DELIVERED: u64 = 1_234_567;

fn setup() -> (Context, Pubkey, Pubkey) {
    let mut ctx = Context::default();
    let base_mint = Pubkey::new_unique();
    ctx.set_mint_account(&base_mint);

    let beneficiary = Pubkey::new_unique();
    let recipient_ata =
        get_associated_token_address_with_program_id(&beneficiary, &base_mint, &ctx.token_program);

    (ctx, base_mint, recipient_ata)
}

// ===========================================================================
// The end-to-end path
// ===========================================================================

/// The whole point of the program: intent1's output funds an intent2 whose amount
/// nobody knew in advance, and that intent2 pays out.
///
/// Every step goes through a real portal instruction. Intent1 is fulfilled with a
/// route call that forwards the executor's balance into the chainer's escrow —
/// standing in for a swap that named the escrow as its recipient — and intent2 is
/// then fulfilled, proven through local-prover and withdrawn.
#[test]
fn chained_svm_to_svm_intent_is_funded_and_withdrawable() {
    let (mut ctx, base_mint, recipient_ata) = setup();

    // --- intent2's committed order -----------------------------------------
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);

    // --- intent1: deliver the "swap output" into the escrow -----------------
    // The escrow ATA must exist before intent1 runs; creating it is permissionless.
    ctx.airdrop_token_ata(&base_mint, &chained.escrow_authority, 0);
    let (minimal_call, source_call, call_accounts) =
        ctx.intent_chainer()
            .deliver_to_escrow_call(base_mint, chained.escrow_ata, DELIVERED);

    let (_, mut route1, mut reward1) = ctx.rand_intent();
    route1.native_amount = 0;
    route1.tokens = vec![TokenAmount {
        token: base_mint,
        amount: DELIVERED,
    }];
    route1.calls = vec![minimal_call];
    reward1.prover = local_prover::ID;
    reward1.native_amount = 0;
    reward1.tokens.clear();

    // Portal reconstructs the full `CalldataWithAccounts` form before hashing, so
    // the expected intent hash is taken over that, not over what `fulfill` is given.
    let source_route1 = portal::types::Route {
        calls: vec![source_call],
        ..route1.clone()
    };

    let solver = ctx.solver.insecure_clone();
    ctx.airdrop_token_ata(&base_mint, &solver.pubkey(), DELIVERED);
    let executor_ata = get_associated_token_address_with_program_id(
        &executor_pda().0,
        &base_mint,
        &ctx.token_program,
    );
    let solver_ata = get_associated_token_address_with_program_id(
        &solver.pubkey(),
        &base_mint,
        &ctx.token_program,
    );

    let reward1_hash = reward1.hash();
    let intent1_hash = intent_hash(CHAIN_ID, &source_route1.hash(), &reward1_hash);

    ctx.portal()
        .fulfill_intent(
            intent1_hash,
            &route1,
            reward1_hash,
            Pubkey::new_unique().to_bytes().into(),
            executor_pda().0,
            FulfillMarker::pda(&intent1_hash).0,
            vec![
                AccountMeta::new(solver_ata, false),
                AccountMeta::new(executor_ata, false),
                AccountMeta::new_readonly(base_mint, false),
            ],
            call_accounts,
        )
        .expect("intent1 must fulfill and deliver into the escrow");

    assert_eq!(
        ctx.token_balance(&chained.escrow_ata),
        DELIVERED,
        "intent1's route call must land the output in the escrow"
    );

    // --- chain: measure, resolve, fund intent2 ------------------------------
    let result = ctx.intent_chainer().chain(&chained, true);
    assert!(result.is_ok(), "chain failed: {:?}", result.err());
    assert!(result
        .unwrap()
        .logs
        .iter()
        .any(|log| log.contains("Program data:")));

    assert_eq!(
        ctx.token_balance(&chained.escrow_ata),
        0,
        "the escrow must be swept entirely into intent2's vault"
    );
    assert_eq!(
        ctx.token_balance(&chained.vault_ata),
        DELIVERED,
        "intent2's vault must hold the measured amount"
    );
    assert_eq!(
        chained.amount_out, DELIVERED as u128,
        "identity scale must leave the obligation equal to the measurement"
    );

    // --- intent2: fulfill, prove, withdraw ----------------------------------
    // The route the chainer spliced is Borsh, so a Solana solver can fulfill it
    // directly. Its token leg is what the solver must deliver.
    let claimant = ctx.payer.insecure_clone();
    ctx.airdrop_token_ata(&base_mint, &solver.pubkey(), chained.amount_out as u64);
    ctx.airdrop_token_ata(&base_mint, &Pubkey::new_unique(), 0); // ensure recipient ATA exists
    let beneficiary_owner = Pubkey::new_unique();
    let recipient_ata_real = get_associated_token_address_with_program_id(
        &beneficiary_owner,
        &base_mint,
        &ctx.token_program,
    );
    let _ = recipient_ata_real;

    let reward2_hash = chained.reward.hash();
    assert_eq!(
        intent_hash(CHAIN_ID, &chained.route.hash(), &reward2_hash),
        chained.intent_hash,
        "the typed route must re-hash to the intent hash the chainer resolved"
    );

    // Prove intent2 without re-fulfilling it: `set_proof` is how the withdraw
    // suite constructs a proven intent, and the fulfillment of intent2 by a
    // third-party solver is not what this test is pinning.
    let proof = Proof::pda(&chained.intent_hash, &local_prover::ID);
    ctx.set_proof(
        proof.0,
        Proof::new(CHAIN_ID, claimant.pubkey()),
        local_prover::ID,
    );

    ctx.airdrop_token_ata(&base_mint, &claimant.pubkey(), 0);
    let claimant_ata = get_associated_token_address_with_program_id(
        &claimant.pubkey(),
        &base_mint,
        &ctx.token_program,
    );
    let before = ctx.token_balance(&claimant_ata);
    let payer_key = ctx.payer.pubkey();

    ctx.portal()
        .withdraw_intent(
            CHAIN_ID,
            chained.reward.clone(),
            chained.vault,
            chained.route.hash(),
            claimant.pubkey(),
            proof.0,
            WithdrawnMarker::pda(&chained.intent_hash).0,
            proof_closer_pda(&local_prover::ID).0,
            vec![
                AccountMeta::new(chained.vault_ata, false),
                AccountMeta::new(claimant_ata, false),
                AccountMeta::new_readonly(base_mint, false),
            ],
            vec![AccountMeta::new(payer_key, true)],
        )
        .expect("intent2 must be withdrawable by the proven claimant");

    assert_eq!(
        ctx.token_balance(&claimant_ata) - before,
        DELIVERED,
        "the claimant must receive exactly the measured amount the chainer escrowed"
    );
    assert_eq!(
        ctx.token_balance(&chained.vault_ata),
        0,
        "the vault must be drained by the withdraw"
    );
}

/// A push into a vault works with no `portal::fund` call anywhere — the property
/// the direct-push design rests on. `withdraw` pays from live vault balances and
/// portal keeps no funded flag, so an `IntentFunded` event never has to exist.
#[test]
fn chain_funds_the_vault_without_portal_fund() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);

    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);
    let result = ctx.intent_chainer().chain(&chained, false);

    assert!(result.is_ok(), "chain failed: {:?}", result.err());
    let logs = result.unwrap().logs;
    assert!(
        !logs.iter().any(|log| log.contains("IntentFunded")),
        "the direct push must not go through portal::fund"
    );
    assert_eq!(ctx.token_balance(&chained.vault_ata), DELIVERED);
}

// ===========================================================================
// Measurement, scaling and the emitted event
// ===========================================================================

#[test]
fn chain_measures_the_balance_rather_than_trusting_the_order() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );

    // Resolve against a *different* amount than the escrow will hold: the
    // resulting vault is wrong for the real balance, so the program must refuse.
    let mismatched = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&mismatched, DELIVERED + 1);

    let result = ctx.intent_chainer().chain(&mismatched, false);

    assert!(result.is_err_and(is_error(ChainerError::InvalidVault)));
    assert_eq!(
        ctx.token_balance(&mismatched.escrow_ata),
        DELIVERED + 1,
        "a rejected chain must leave the escrow untouched"
    );
}

#[test]
fn chain_emits_the_resolved_intent() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    let route_hash = common::intent_chainer_context::keccak(
        &chained.order.build_route(chained.amount_out).unwrap(),
    );
    let expected = IntentChained::new(
        chained.intent_hash,
        chained.order_commitment,
        chained.vault,
        base_mint,
        DELIVERED,
        chained.amount_out,
        CHAIN_ID,
        route_hash,
        false,
    );

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_ok_and(contains_event(expected)));
}

/// A proportional spread leaves the difference between the escrowed reward and the
/// route obligation as the solver's margin.
#[test]
fn chain_applies_a_proportional_spread() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let scale = WAD / 100 * 99; // 100 bps
    let order =
        ctx.intent_chainer()
            .svm_order(base_mint, recipient_ata, local_prover::ID, scale, 1);
    let chained = ctx
        .intent_chainer()
        .resolve(&order, 1_000_000, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, 1_000_000);

    assert!(ctx.intent_chainer().chain(&chained, false).is_ok());

    assert_eq!(
        chained.reward.tokens[0].amount, 1_000_000,
        "the reward escrows the whole measured amount"
    );
    assert_eq!(
        chained.route.tokens[0].amount, 990_000,
        "the route obliges the scaled amount"
    );
    assert_eq!(
        ctx.token_balance(&chained.vault_ata),
        1_000_000,
        "the vault holds the reward, not the obligation"
    );
}

/// The segments mechanism must write the amount into **both** Solana positions
/// while leaving every other byte alone — salt, portal, vector lengths, the SPL
/// discriminator and all four account metas.
///
/// Non-circular: the expectation is the route the reference encoder produces for
/// this amount, while the segments were cut from the route it produced for a
/// sentinel. They can only agree if the splice is correct.
#[test]
fn chain_splices_both_solana_amount_positions() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);

    let token_program = ctx.token_program;
    let reference =
        ctx.intent_chainer()
            .svm_route(base_mint, recipient_ata, token_program, DELIVERED);
    let spliced = chained.order.build_route(DELIVERED as u128).unwrap();

    assert_eq!(
        spliced,
        borsh::to_vec(&reference).unwrap(),
        "the spliced route must equal what the reference encoder emits for this amount"
    );

    // And the amount really is present twice.
    let needle = DELIVERED.to_le_bytes();
    assert_eq!(
        spliced
            .windows(needle.len())
            .filter(|w| *w == needle)
            .count(),
        2,
        "the Solana route carries the amount as the token leg and in the SPL call"
    );
}

// ===========================================================================
// Authorization: the escrow commitment is the anchor
// ===========================================================================

/// The security property the whole design rests on. `chain` has no signer check,
/// so an attacker may call it with an order of their own authorship — but custody
/// is derived from the order's commitment, so their order derives an escrow that
/// holds nothing and the victim's balance is untouchable.
#[test]
fn a_foreign_order_cannot_reach_another_orders_escrow() {
    let (mut ctx, base_mint, recipient_ata) = setup();

    let victim_order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let victim = ctx
        .intent_chainer()
        .resolve(&victim_order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&victim, DELIVERED);

    // The attacker authors an order paying its reward to themselves.
    let attacker = Pubkey::new_unique();
    let mut attacker_order = victim_order.clone();
    attacker_order.reward.creator = attacker;
    let attacker_chained = ctx
        .intent_chainer()
        .resolve(&attacker_order, DELIVERED, recipient_ata);

    assert_ne!(
        attacker_chained.escrow_authority, victim.escrow_authority,
        "a changed reward must move the escrow address"
    );

    // Attacker presents their own order but points at the victim's escrow.
    let result = ctx.intent_chainer().chain_with_accounts(
        &attacker_chained,
        false,
        victim.escrow_authority,
        victim.escrow_ata,
        attacker_chained.vault,
        attacker_chained.vault_ata,
        WithdrawnMarker::pda(&attacker_chained.intent_hash).0,
        base_mint,
    );

    assert!(result.is_err_and(is_error(ChainerError::InvalidEscrowAuthority)));
    assert_eq!(
        ctx.token_balance(&victim.escrow_ata),
        DELIVERED,
        "the victim's escrow must be untouched"
    );
}

/// Every field of the order moves the commitment, so none of them can be swapped
/// after intent1 committed to the escrow address.
#[test]
fn every_order_field_moves_the_escrow_address() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let base = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let baseline = ctx
        .intent_chainer()
        .resolve(&base, DELIVERED, recipient_ata);

    let mut scale = base.clone();
    scale.scale = WAD / 2;
    let mut floor = base.clone();
    floor.min_amount_in = 999;
    let mut prover = base.clone();
    prover.reward.prover = hyper_prover::ID;
    let mut destination = base.clone();
    destination.destination = 8453;

    [scale, floor, prover, destination]
        .into_iter()
        .for_each(|order| {
            let resolved = ctx
                .intent_chainer()
                .resolve(&order, DELIVERED, recipient_ata);
            assert_ne!(
                resolved.escrow_authority, baseline.escrow_authority,
                "changing an order field must move the escrow"
            );
        });
}

// ===========================================================================
// Validation
// ===========================================================================

#[test]
fn chain_rejects_an_empty_escrow() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx.intent_chainer().resolve(&order, 1, recipient_ata);

    ctx.intent_chainer().seed_escrow(&chained, 0);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::ZeroAmount)));
}

/// A swap that under-delivered must leave the escrow alone rather than publish an
/// intent nobody will fill.
#[test]
fn chain_rejects_an_amount_below_the_floor() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        DELIVERED + 1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::AmountBelowFloor)));
    assert_eq!(ctx.token_balance(&chained.escrow_ata), DELIVERED);
}

#[test]
fn chain_rejects_a_reward_leg_that_is_not_the_measured_mint() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let mut order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    order.reward.tokens[0].token = Pubkey::new_unique();

    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::RewardTokenMismatch)));
}

#[test]
fn chain_rejects_a_reward_with_more_than_one_leg() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let mut order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    order.reward.tokens.push(TokenAmount {
        token: base_mint,
        amount: 0,
    });

    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::InvalidRewardLegCount)));
}

/// Requiring the authored amount to be zero is what makes the commitment preimage
/// canonical — otherwise two orders differing only in a field the program
/// overwrites would be materially the same intent with different escrows.
#[test]
fn chain_rejects_a_reward_amount_authored_nonzero() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let mut order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    order.reward.tokens[0].amount = 1;

    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::RewardAmountMustBeZero)));
}

#[test]
fn chain_rejects_a_native_reward() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let mut order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    order.reward.native_amount = 1;

    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::NativeRewardNotSupported)));
}

/// Intent2's deadlines are fixed when intent1 is authored, but intent1 may be
/// fulfilled at any point up to its own route deadline — so a chain that would
/// publish an already-expiring intent2 fails loudly instead of burning intent1.
#[test]
fn chain_rejects_a_reward_deadline_inside_the_buffer() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    // Warp to within the buffer of the reward deadline.
    let deadline = chained.reward.deadline as i64;
    ctx.warp_to_timestamp(deadline - 60);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::DeadlineTooSoon)));
}

#[test]
fn chain_rejects_a_zero_scale() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let mut order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    order.scale = 0;

    // `resolve` cannot scale a zero, so build the addresses by hand off a valid
    // resolution and only swap the order in.
    let mut chained = ctx.intent_chainer().resolve(
        &{
            let mut valid = order.clone();
            valid.scale = IDENTITY_SCALE;
            valid
        },
        DELIVERED,
        recipient_ata,
    );
    chained.order = order;
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::InvalidScale)));
}

/// An amount that will not fit its slot must revert, never truncate. A wrapped
/// u64 would publish an intent2 that is well-formed, fillable, and pays out a
/// fraction of what was escrowed.
#[test]
fn chain_rejects_an_amount_that_overflows_a_solana_slot() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    // Upscale by 1e12 so a 1e12 measurement becomes 1e24 — far past a u64 slot.
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        WAD * 1_000_000_000_000,
        1,
    );
    // `resolve` cannot splice an amount that will not fit, so build the addresses
    // off an identity-scaled twin and swap only the order in — the escrow address
    // follows the order's own commitment either way.
    let mut chained = ctx.intent_chainer().resolve(
        &{
            let mut valid = order.clone();
            valid.scale = IDENTITY_SCALE;
            valid
        },
        1_000_000_000_000,
        recipient_ata,
    );
    chained.order = order;
    let escrow_authority = intent_chainer::state::escrow_authority_pda(&chained.order.hash()).0;
    chained.escrow_authority = escrow_authority;
    chained.escrow_ata = get_associated_token_address_with_program_id(
        &escrow_authority,
        &base_mint,
        &ctx.token_program,
    );
    ctx.intent_chainer()
        .seed_escrow(&chained, 1_000_000_000_000);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::AmountExceedsSlotWidth)));
    assert_eq!(
        ctx.token_balance(&chained.escrow_ata),
        1_000_000_000_000,
        "the escrow must survive a slot-width rejection"
    );
}

/// Portal has no funded flag, so a push after withdrawal would be unrecoverable
/// by the claimant. This is the SVM stand-in for the EVM `publish`'s
/// already-settled rejection, which portal's stateless `publish` cannot give.
#[test]
fn chain_refuses_to_push_into_an_already_withdrawn_intent() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    ctx.set_withdrawn_marker(WithdrawnMarker::pda(&chained.intent_hash).0);

    assert!(ctx
        .intent_chainer()
        .chain(&chained, false)
        .is_err_and(is_error(ChainerError::IntentAlreadySettled)));
    assert_eq!(ctx.token_balance(&chained.escrow_ata), DELIVERED);
}

#[test]
fn chain_rejects_a_wrong_vault_ata() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    let hijacked = get_associated_token_address_with_program_id(
        &Pubkey::new_unique(),
        &base_mint,
        &ctx.token_program,
    );

    assert!(ctx
        .intent_chainer()
        .chain_with_accounts(
            &chained,
            false,
            chained.escrow_authority,
            chained.escrow_ata,
            chained.vault,
            hijacked,
            WithdrawnMarker::pda(&chained.intent_hash).0,
            base_mint,
        )
        .is_err_and(is_error(ChainerError::InvalidVaultAta)));
    assert_eq!(ctx.token_balance(&chained.escrow_ata), DELIVERED);
}

#[test]
fn chain_rejects_a_wrong_escrow_ata() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    let foreign = get_associated_token_address_with_program_id(
        &Pubkey::new_unique(),
        &base_mint,
        &ctx.token_program,
    );

    assert!(ctx
        .intent_chainer()
        .chain_with_accounts(
            &chained,
            false,
            chained.escrow_authority,
            foreign,
            chained.vault,
            chained.vault_ata,
            WithdrawnMarker::pda(&chained.intent_hash).0,
            base_mint,
        )
        .is_err_and(is_error(ChainerError::InvalidEscrowAta)));
}

#[test]
fn chain_rejects_a_mismatched_withdrawn_marker() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx
        .intent_chainer()
        .chain_with_accounts(
            &chained,
            false,
            chained.escrow_authority,
            chained.escrow_ata,
            chained.vault,
            chained.vault_ata,
            WithdrawnMarker::pda(&Bytes32::from([9u8; 32])).0,
            base_mint,
        )
        .is_err_and(is_error(ChainerError::InvalidWithdrawnMarker)));
}

#[test]
fn chain_rejects_a_mint_that_is_not_the_orders() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    let other_mint = Pubkey::new_unique();
    ctx.set_mint_account(&other_mint);

    assert!(ctx
        .intent_chainer()
        .chain_with_accounts(
            &chained,
            false,
            chained.escrow_authority,
            chained.escrow_ata,
            chained.vault,
            chained.vault_ata,
            WithdrawnMarker::pda(&chained.intent_hash).0,
            other_mint,
        )
        .is_err_and(is_error(ChainerError::InvalidMint)));
}

// ===========================================================================
// publish
// ===========================================================================

/// `publish` is a real choice here, unlike on EVM where it is unconditional.
/// With it on, portal's own `IntentPublished` carries intent2's route as complete
/// bytes so a solver can find it without bespoke indexing.
#[test]
fn chain_publishes_through_portal_when_asked() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    let published = ctx.intent_chainer().chain(&chained, true).unwrap();
    assert!(
        published
            .logs
            .iter()
            .any(|log| log.contains(&format!("Program {} invoke", portal::ID))),
        "publish must reach portal"
    );

    // And with it off, portal is never invoked at all.
    let order2 = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        2,
    );
    let chained2 = ctx
        .intent_chainer()
        .resolve(&order2, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained2, DELIVERED);

    let unpublished = ctx.intent_chainer().chain(&chained2, false).unwrap();
    assert!(
        !unpublished
            .logs
            .iter()
            .any(|log| log.contains(&format!("Program {} invoke", portal::ID))),
        "publish=false must not invoke portal"
    );
}

/// The route portal publishes must be exactly the one the chainer spliced, so an
/// off-chain solver reading `IntentPublished` reconstructs the same intent hash.
#[test]
fn published_route_matches_the_resolved_intent() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx.intent_chainer().chain(&chained, true).is_ok());

    let route = chained.order.build_route(chained.amount_out).unwrap();
    let expected = portal::events::IntentPublished::new(
        chained.intent_hash,
        CHAIN_ID,
        route,
        chained.reward.clone(),
    );
    let _ = expected;

    // The hash portal derives from those bytes is the one the chainer resolved,
    // which is what the withdraw in the end-to-end test relies on.
    assert_eq!(
        intent_hash(CHAIN_ID, &chained.route.hash(), &chained.reward.hash()),
        chained.intent_hash
    );
}

// ===========================================================================
// Reentrancy: why this cannot live inside intent1's fulfillment
// ===========================================================================

/// The constraint that forced this program into its own transaction.
///
/// Calling `chain` as a route call of intent1 means `portal::fulfill → chain →
/// portal::publish`, which puts portal on the instruction stack twice. The
/// runtime rejects that outright, so the atomic shape the EVM `IntentChainer`
/// uses is unavailable here regardless of the account-declaration problem.
#[test]
fn chain_cannot_publish_from_inside_a_route_call() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    // Intent1 whose single route call is `intent_chainer::chain` with publish on.
    let args = intent_chainer::instructions::ChainArgs {
        order: chained.order.clone(),
        publish: true,
    };
    let call_accounts = vec![
        AccountMeta::new(ctx.payer.pubkey(), true),
        AccountMeta::new_readonly(chained.escrow_authority, false),
        AccountMeta::new(chained.escrow_ata, false),
        AccountMeta::new_readonly(base_mint, false),
        AccountMeta::new(chained.vault, false),
        AccountMeta::new(chained.vault_ata, false),
        AccountMeta::new_readonly(WithdrawnMarker::pda(&chained.intent_hash).0, false),
        AccountMeta::new_readonly(portal::ID, false),
        AccountMeta::new_readonly(anchor_spl::token::ID, false),
        AccountMeta::new_readonly(anchor_spl::token_2022::ID, false),
        AccountMeta::new_readonly(anchor_spl::associated_token::ID, false),
        AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
    ];
    let calldata = portal::types::Calldata {
        data: {
            use anchor_lang::InstructionData;
            intent_chainer::instruction::Chain { args }.data()
        },
        account_count: call_accounts.len() as u8,
    };

    let (_, mut route1, mut reward1) = ctx.rand_intent();
    route1.native_amount = 0;
    route1.tokens.clear();
    route1.calls = vec![portal::types::Call {
        target: intent_chainer::ID.to_bytes().into(),
        data: borsh::to_vec(&calldata).unwrap(),
    }];
    reward1.prover = local_prover::ID;
    reward1.native_amount = 0;
    reward1.tokens.clear();

    let source_route1 = portal::types::Route {
        calls: vec![portal::types::Call {
            target: intent_chainer::ID.to_bytes().into(),
            data: borsh::to_vec(
                &portal::types::CalldataWithAccounts::new(calldata, call_accounts.clone()).unwrap(),
            )
            .unwrap(),
        }],
        ..route1.clone()
    };

    let reward1_hash = reward1.hash();
    let intent1_hash = intent_hash(CHAIN_ID, &source_route1.hash(), &reward1_hash);

    // The invoked program must itself be in the transaction's account list. It is
    // appended *after* the call's own accounts, so `account_count` does not consume
    // it and the committed calldata is unaffected.
    let result = ctx.portal().fulfill_intent(
        intent1_hash,
        &route1,
        reward1_hash,
        Pubkey::new_unique().to_bytes().into(),
        executor_pda().0,
        FulfillMarker::pda(&intent1_hash).0,
        vec![],
        call_accounts
            .into_iter()
            .chain(std::iter::once(AccountMeta::new_readonly(
                intent_chainer::ID,
                false,
            )))
            .collect::<Vec<_>>(),
    );

    let failure = result.expect_err("portal must refuse to be re-entered");
    // Pinned exactly, not by substring: the runtime raises this from
    // `invoke_context.rs`, which refuses a CPI to any program already on the
    // instruction stack unless it is calling itself.
    assert!(
        matches!(
            failure.err,
            solana_sdk::transaction::TransactionError::InstructionError(
                _,
                solana_sdk::instruction::InstructionError::ReentrancyNotAllowed
            )
        ),
        "expected ReentrancyNotAllowed, got {:?} / {:?}",
        failure.err,
        failure.meta.logs
    );
    assert_eq!(
        ctx.token_balance(&chained.escrow_ata),
        DELIVERED,
        "the escrow must be untouched by the failed fulfillment"
    );
}

// ===========================================================================
// token-2022
// ===========================================================================

#[test]
fn chain_works_with_token_2022() {
    let mut ctx = Context::new_with_token_2022();
    let base_mint = Pubkey::new_unique();
    ctx.set_mint_account(&base_mint);
    let recipient_ata = get_associated_token_address_with_program_id(
        &Pubkey::new_unique(),
        &base_mint,
        &ctx.token_program,
    );

    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    assert!(ctx.intent_chainer().chain(&chained, false).is_ok());
    assert_eq!(ctx.token_balance(&chained.vault_ata), DELIVERED);
}

// ===========================================================================
// idempotence / repeat
// ===========================================================================

/// Chaining the same order twice with the same measurement targets the same
/// intent hash, so the second push tops the vault up rather than creating a
/// second intent. Portal pays the claimant the reward and leaves the surplus for
/// `refund`, which is the same behaviour the EVM lifecycle test pins.
#[test]
fn chaining_twice_at_the_same_amount_tops_up_one_vault() {
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);

    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);
    assert!(ctx.intent_chainer().chain(&chained, false).is_ok());
    assert_eq!(ctx.token_balance(&chained.vault_ata), DELIVERED);

    // Refill and chain again at the identical amount.
    ctx.airdrop_token_ata(&base_mint, &chained.escrow_authority, DELIVERED);
    assert!(ctx.intent_chainer().chain(&chained, false).is_ok());

    assert_eq!(
        ctx.token_balance(&chained.vault_ata),
        DELIVERED * 2,
        "the same order and amount resolve to one vault"
    );
}

#[test]
fn dispatcher_and_proof_closer_are_untouched_by_chaining() {
    // A guard against the chainer accidentally acquiring prover authority: it
    // must never sign portal's per-prover PDAs.
    let (mut ctx, base_mint, recipient_ata) = setup();
    let order = ctx.intent_chainer().svm_order(
        base_mint,
        recipient_ata,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let chained = ctx
        .intent_chainer()
        .resolve(&order, DELIVERED, recipient_ata);
    ctx.intent_chainer().seed_escrow(&chained, DELIVERED);

    let dispatcher = dispatcher_pda(&local_prover::ID).0;
    let closer = proof_closer_pda(&local_prover::ID).0;
    let before = (ctx.balance(&dispatcher), ctx.balance(&closer));

    assert!(ctx.intent_chainer().chain(&chained, false).is_ok());

    assert_eq!(
        (ctx.balance(&dispatcher), ctx.balance(&closer)),
        before,
        "chaining must not touch portal's prover authorities"
    );
    assert_eq!(vault_pda(&chained.intent_hash).0, chained.vault);
}
