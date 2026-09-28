use std::iter;

use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::{AnchorSerialize, Discriminator, Space};
use eco_svm_std::prover::{self, IntentHashClaimant, Proof, ProofData};
use eco_svm_std::{Bytes32, CHAIN_ID};
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::event::evm_address_to_bytes32;
use polymer_prover::instructions::{
    PolymerProverError, MAX_INTENTS_PER_PROVE, MAX_PAIRS_PER_VALIDATE_LEGACY_TX,
};
use polymer_prover::polymer;
use polymer_prover::state::{Config, ProofAccount};
use portal::events::IntentWithdrawn;
use portal::state::{proof_closer_pda, vault_pda, WithdrawnMarker};
use portal::types::{intent_hash, Reward};
use rand::random;
use solana_packet::PACKET_DATA_SIZE;
use solana_sdk::instruction::InstructionError;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::TransactionError;
use solana_transaction_context::MAX_INSTRUCTION_TRACE_LENGTH;

use crate::common::polymer_prover_context::{intent_fulfilled_result, PolymerProver};

pub mod common;

/// Whitelisted EVM PolymerProver address used by every test.
const EMITTER: [u8; 20] = [0xec; 20];
/// EVM chain the fulfilments happened on.
const EVM_CHAIN_ID: u32 = 8453;
/// LiteSVM's default fee structure charges 5000 lamports per signature; the
/// `validate` transaction carries exactly one (the authority's).
const TRANSACTION_FEE: u64 = 5_000;

/// `validate`'s instruction-trace model against `MAX_INSTRUCTION_TRACE_LENGTH`
/// (64), the ceiling `validate.rs::mark_intent_hashes_proven` documents: two
/// top-level instructions (ComputeBudget + `validate`) plus the `validate_event`
/// CPI, then per pair the CPIs `AccountExt::init` makes for the Proof PDA plus
/// `emit_cpi!`. The ceiling tests derive every threshold from these and pin it
/// to the number validate.rs quotes, so a change in `create_account`'s CPI
/// count fails at the assertion naming the doc line it invalidates.
const FIXED_TRACE_ENTRIES: usize = 3;
/// `create_account` + `emit_cpi!`.
const TRACE_ENTRIES_PER_FRESH_PAIR: usize = 2;
/// Pre-funded below the rent-exempt minimum: `transfer + allocate + assign` +
/// `emit_cpi!`.
const TRACE_ENTRIES_PER_GRIEFED_PAIR: usize = 4;
/// Pre-funded at or above the rent-exempt minimum: `allocate + assign` +
/// `emit_cpi!`.
const TRACE_ENTRIES_PER_FUNDED_PAIR: usize = 3;
/// Under a Proof account's rent-exempt minimum (~1.22M for 48 bytes); a bare
/// 1-lamport airdrop is rejected by litesvm's rent check.
const UNDER_RENT_LAMPORTS: u64 = 1_000_000;

struct Fixture {
    ctx: common::Context,
    authority: Keypair,
}

fn setup() -> Fixture {
    let mut ctx = common::Context::default();
    ctx.polymer_prover()
        .init(vec![evm_address_to_bytes32(EMITTER)], Config::pda().0)
        .unwrap();
    let authority = Keypair::new();
    ctx.polymer_prover()
        .polymer_create_accounts(&authority)
        .unwrap();

    Fixture { ctx, authority }
}

fn rand_pairs(count: usize) -> Vec<IntentHashClaimant> {
    (0..count)
        .map(|_| {
            IntentHashClaimant::new(
                random::<[u8; 32]>().into(),
                Pubkey::new_unique().to_bytes().into(),
            )
        })
        .collect()
}

fn proof_metas(pairs: &[IntentHashClaimant]) -> Vec<AccountMeta> {
    pairs
        .iter()
        .map(|pair| AccountMeta::new(Proof::pda(&pair.intent_hash, &polymer_prover::ID).0, false))
        .collect()
}

/// A well-formed event for `pairs`, source = this chain, destination = EVM chain.
fn event_for(pairs: &[IntentHashClaimant]) -> ValidationResultAccount {
    intent_fulfilled_result(
        EMITTER,
        CHAIN_ID,
        EVM_CHAIN_ID,
        ProofData::new(EVM_CHAIN_ID.into(), pairs.to_vec()).to_bytes(),
    )
}

fn load_and_validate(
    fixture: &mut Fixture,
    event: &ValidationResultAccount,
    proof_accounts: Vec<AccountMeta>,
) -> common::TransactionResult {
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&fixture.authority, event)
        .unwrap();
    fixture
        .ctx
        .polymer_prover()
        .validate(&fixture.authority, proof_accounts)
}

/// Loads a well-formed event for `pairs` and sends `validate` with the fixed
/// slots in `accounts`, so a test can point one slot at the wrong account while
/// every other precondition stays valid.
fn load_and_validate_with_accounts(
    fixture: &mut Fixture,
    pairs: &[IntentHashClaimant],
    accounts: polymer_prover::accounts::Validate,
) -> common::TransactionResult {
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&fixture.authority, &event_for(pairs))
        .unwrap();
    fixture.ctx.polymer_prover().validate_with_accounts(
        &fixture.authority,
        accounts,
        proof_metas(pairs),
    )
}

/// `validate` over `total` pairs with the first `prefunded` Proof PDAs seeded
/// with `lamports`, so a test can pick the fresh / under-rent / at-rent path
/// per pair.
fn validate_with_prefunded(
    total: usize,
    prefunded: usize,
    lamports: u64,
) -> (Fixture, Vec<IntentHashClaimant>, common::TransactionResult) {
    let mut fixture = setup();
    let pairs = rand_pairs(total);
    for meta in proof_metas(&pairs).into_iter().take(prefunded) {
        fixture.ctx.airdrop(&meta.pubkey, lamports).unwrap();
    }
    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));
    (fixture, pairs, result)
}

fn proof_exists(fixture: &Fixture, pair: &IntentHashClaimant) -> bool {
    fixture
        .ctx
        .account::<ProofAccount>(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
        .is_some()
}

fn assert_all_proven(fixture: &Fixture, pairs: &[IntentHashClaimant]) {
    assert!(pairs.iter().all(|pair| proof_exists(fixture, pair)));
}

/// Nothing partially proven: a pre-funded PDA may still hold its lamports but
/// carries no Proof.
fn assert_none_proven(fixture: &Fixture, pairs: &[IntentHashClaimant]) {
    assert!(!pairs.iter().any(|pair| proof_exists(fixture, pair)));
}

/// The whole event reverted on the instruction-trace ceiling; index 1 is
/// `validate` (index 0 is the ComputeBudget instruction).
fn assert_trace_exceeded(result: common::TransactionResult) {
    assert_eq!(
        result.unwrap_err().err,
        TransactionError::InstructionError(1, InstructionError::MaxInstructionTraceLengthExceeded)
    );
}

#[test]
fn validate_success() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&fixture.authority, &event_for(&pairs))
        .unwrap();
    // Snapshot after the load transactions so the delta covers only `validate`.
    let authority_balance_before = fixture.ctx.balance(&fixture.authority.pubkey());

    let result = fixture
        .ctx
        .polymer_prover()
        .validate(&fixture.authority, proof_metas(&pairs));

    let claimant = Pubkey::new_from_array(pairs[0].claimant.into());
    assert!(
        result.is_ok_and(common::contains_cpi_event(prover::IntentProven::new(
            pairs[0].intent_hash,
            claimant,
            EVM_CHAIN_ID.into(),
        )))
    );
    let proof_pda = Proof::pda(&pairs[0].intent_hash, &polymer_prover::ID).0;
    let proof: ProofAccount = fixture.ctx.account(&proof_pda).unwrap();
    assert_eq!(proof.0.destination, u64::from(EVM_CHAIN_ID));
    assert_eq!(proof.0.claimant, claimant);
    // The relayer, and no other account, funded the Proof rent: its spend is
    // exactly the Proof's lamports plus the single-signature fee.
    assert_eq!(
        authority_balance_before - fixture.ctx.balance(&fixture.authority.pubkey()),
        fixture.ctx.balance(&proof_pda) + TRANSACTION_FEE
    );
    // Polymer cleared the cache.
    let cache = fixture
        .ctx
        .account::<mock_polymer_prover::ProofCacheAccount>(
            &polymer::cache_pda(&fixture.authority.pubkey()).0,
        )
        .unwrap();
    assert!(cache.cache.is_empty());
}

#[test]
fn validate_multiple_success() {
    let mut fixture = setup();
    let pairs = rand_pairs(3);

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));
    assert!(result.is_ok());

    for pair in &pairs {
        let proof: ProofAccount = fixture
            .ctx
            .account(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
            .unwrap();
        assert_eq!(
            proof.0.claimant,
            Pubkey::new_from_array(pair.claimant.into())
        );
        assert!(result
            .clone()
            .is_ok_and(common::contains_cpi_event(prover::IntentProven::new(
                pair.intent_hash,
                Pubkey::new_from_array(pair.claimant.into()),
                EVM_CHAIN_ID.into(),
            ))));
    }
}

/// The EVM twin skips pairs this side records. `PolymerProver.sol` skips
/// `claimantBytes >> 160 != 0` (bytes32->address narrowing) and `BaseProver`
/// skips a zero claimant; `validate` records any 32 bytes verbatim, so the
/// reward is stranded — `withdraw` pays a key nobody can spend from and
/// `refund` refuses while the Proof exists
/// (`refund.rs::refund_intent_fulfilled_and_not_withdrawn_fail` pins that gate;
/// `validate_withdraw_success` pins the layout Portal reads). Deliberate: see
/// the note in `validate.rs::mark_intent_hash_proven` and spec section 3.4
/// step 5; hyper-prover and local-prover behave the same way. `rand_pairs`'
/// `Pubkey::new_unique()` already carries a non-zero high half, so every
/// happy-path test covers the `>> 160 != 0` shape; only the all-zero claimant
/// is new here. Pinned so aligning with the EVM leg has to break a test rather
/// than silently change cross-chain behaviour.
#[test]
fn validate_records_claimant_the_evm_leg_would_skip() {
    let mut fixture = setup();
    let pair = IntentHashClaimant::new(random::<[u8; 32]>().into(), [0u8; 32].into());
    let pairs = vec![pair];

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));

    // No skip and no error: the Proof exists with the claimant as given.
    let claimant = Pubkey::new_from_array(pairs[0].claimant.into());
    assert_eq!(claimant, Pubkey::default());
    assert!(
        result.is_ok_and(common::contains_cpi_event(prover::IntentProven::new(
            pairs[0].intent_hash,
            claimant,
            EVM_CHAIN_ID.into(),
        )))
    );
    let proof: ProofAccount = fixture
        .ctx
        .account(&Proof::pda(&pairs[0].intent_hash, &polymer_prover::ID).0)
        .unwrap();
    assert_eq!(proof.0.destination, u64::from(EVM_CHAIN_ID));
    assert_eq!(proof.0.claimant, claimant);
}

/// Drives a fresh `count`-pair event through `validate` and asserts every
/// Proof landed with its `(destination, claimant)` and one `IntentProven` per
/// pair; returns the compute units the transaction consumed.
fn validate_full_batch(count: usize) -> u64 {
    let mut fixture = setup();
    let pairs = rand_pairs(count);

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs)).unwrap();

    for pair in &pairs {
        let proof: ProofAccount = fixture
            .ctx
            .account(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
            .unwrap();
        let claimant = Pubkey::new_from_array(pair.claimant.into());
        assert_eq!(proof.0.destination, u64::from(EVM_CHAIN_ID));
        assert_eq!(proof.0.claimant, claimant);
        let proven = common::count_cpi_events(prover::IntentProven::new(
            pair.intent_hash,
            claimant,
            EVM_CHAIN_ID.into(),
        ))(result.clone());
        assert_eq!(
            proven, 1,
            "one IntentProven per pair; logs: {:#?}",
            result.logs
        );
    }
    result.compute_units_consumed
}

/// The inbound counterpart of `prove_via_portal_at_max_intents_emits_every_log_untruncated`:
/// the batch size the outbound cap tells EVM `Inbox.prove` to send must land
/// in one `validate`, and within our share of the compute budget.
#[test]
fn validate_at_max_intents_per_prove_succeeds() {
    let cu = validate_full_batch(MAX_INTENTS_PER_PROVE);
    assert!(
        cu <= u64::from(PolymerProver::OUR_VALIDATE_CU_BUDGET),
        "validate at {MAX_INTENTS_PER_PROVE} pairs consumed {cu} CU"
    );
}

/// The largest batch a legacy transaction can deliver (see
/// `validate_legacy_transaction_fits_only_23_pairs` for the packet arithmetic).
#[test]
fn validate_at_max_legacy_batch_writes_every_proof() {
    let cu = validate_full_batch(MAX_PAIRS_PER_VALIDATE_LEGACY_TX);
    assert!(cu <= u64::from(PolymerProver::OUR_VALIDATE_CU_BUDGET));
}

/// Pins our own marginal cost per pair, and the total at a full batch, as a
/// regression guard. The mock's `validate_event` is nearly free, so this is
/// NOT evidence that a real 24-pair `validate` fits in 1.4M CU — that headroom
/// is what `validate_polymer_prover_real.rs` measures against Polymer's binary.
/// Measured ~9-10k CU per pair and ~252k total at 24; the bounds leave slack
/// because CU counts move on Anchor and toolchain bumps.
#[test]
fn validate_per_pair_compute_cost_is_bounded() {
    let cu_one = validate_full_batch(1);
    let cu_max = validate_full_batch(MAX_INTENTS_PER_PROVE);

    let per_pair = (cu_max - cu_one) / (MAX_INTENTS_PER_PROVE as u64 - 1);
    assert!(
        per_pair < PolymerProver::PER_PAIR_CU_BOUND,
        "per-pair cost {per_pair} CU (1 pair: {cu_one}, {MAX_INTENTS_PER_PROVE} pairs: {cu_max})"
    );
    // The subtraction above would smuggle growth of the mock's own decode (its
    // cache grows with the event) into "per pair"; bound the total separately.
    assert!(
        cu_max <= u64::from(PolymerProver::OUR_VALIDATE_CU_BUDGET),
        "validate at {MAX_INTENTS_PER_PROVE} pairs consumed {cu_max} CU"
    );
}

/// The real ceiling on pairs per event is the 64-entry instruction trace, not
/// account locks or compute. Derived rather than written as 30/31 so a runtime
/// bump to `MAX_INSTRUCTION_TRACE_LENGTH` moves the test with it.
#[test]
fn validate_batch_ceiling_is_the_instruction_trace_limit() {
    let max_fresh =
        (MAX_INSTRUCTION_TRACE_LENGTH - FIXED_TRACE_ENTRIES) / TRACE_ENTRIES_PER_FRESH_PAIR;
    assert_eq!(max_fresh, 30, "validate.rs quotes 30 fresh pairs");

    validate_full_batch(max_fresh);

    // One more pair and the whole event reverts; nothing is partially proven.
    let (fixture, pairs, result) = validate_with_prefunded(max_fresh + 1, 0, 0);
    assert_trace_exceeded(result);
    assert_none_proven(&fixture, &pairs);
}

/// The only batch limit with an adversary behind it. A pre-funded Proof PDA
/// sends `create_account` down its griefing-resistant `transfer + allocate +
/// assign` path, which costs 4 trace entries per pair instead of 2, so griefing
/// 7 of a 24-pair batch is enough to push it over the ceiling:
/// 3 + 2(24 - k) + 4k <= 64 gives k <= 6. Both shapes are pinned at their
/// boundary — an all-griefed batch (the doc's "15") and the mixed batch the
/// formula describes — with every threshold derived, not written. The
/// relayer's recovery is a smaller re-prove on EVM, not a lost intent.
#[test]
fn validate_prefunded_proof_pdas_lower_the_batch_ceiling() {
    // All griefed: the ceiling drops from 30 to 15.
    let max_all_griefed =
        (MAX_INSTRUCTION_TRACE_LENGTH - FIXED_TRACE_ENTRIES) / TRACE_ENTRIES_PER_GRIEFED_PAIR;
    assert_eq!(
        max_all_griefed, 15,
        "validate.rs quotes 15 pre-funded pairs"
    );

    let (fixture, pairs, result) =
        validate_with_prefunded(max_all_griefed, max_all_griefed, UNDER_RENT_LAMPORTS);
    result.unwrap();
    assert_all_proven(&fixture, &pairs);

    let (fixture, pairs, result) = validate_with_prefunded(
        max_all_griefed + 1,
        max_all_griefed + 1,
        UNDER_RENT_LAMPORTS,
    );
    assert_trace_exceeded(result);
    assert_none_proven(&fixture, &pairs);

    // Partially griefed: how many pre-funded PDAs a full inbound batch tolerates.
    // `saturating_sub` so a future cap past 30 fails the assertion below rather
    // than panicking on underflow.
    let budget = MAX_INSTRUCTION_TRACE_LENGTH
        .saturating_sub(FIXED_TRACE_ENTRIES + TRACE_ENTRIES_PER_FRESH_PAIR * MAX_INTENTS_PER_PROVE);
    let max_griefed_in_full_batch =
        budget / (TRACE_ENTRIES_PER_GRIEFED_PAIR - TRACE_ENTRIES_PER_FRESH_PAIR);
    assert_eq!(
        max_griefed_in_full_batch, 6,
        "validate.rs quotes k <= 6 at a 24-pair batch"
    );

    let (fixture, pairs, result) = validate_with_prefunded(
        MAX_INTENTS_PER_PROVE,
        max_griefed_in_full_batch,
        UNDER_RENT_LAMPORTS,
    );
    result.unwrap();
    assert_all_proven(&fixture, &pairs);

    let (fixture, pairs, result) = validate_with_prefunded(
        MAX_INTENTS_PER_PROVE,
        max_griefed_in_full_batch + 1,
        UNDER_RENT_LAMPORTS,
    );
    assert_trace_exceeded(result);
    assert_none_proven(&fixture, &pairs);
}

/// The cheaper grief: a Proof PDA pre-funded at or above the rent-exempt
/// minimum skips the transfer (`allocate + assign` only), so 3 entries per pair
/// and a ceiling of 20. `create_account`'s `data_is_empty() && owner !=
/// program_id` guard still holds for a lamport-only system account, so this
/// stays on the allocate-and-assign path rather than the fresh one.
#[test]
fn validate_rent_exempt_prefunded_proof_pdas_lower_the_batch_ceiling_to_20() {
    let rent = Rent::default().minimum_balance(8 + ProofAccount::INIT_SPACE);
    let max_funded =
        (MAX_INSTRUCTION_TRACE_LENGTH - FIXED_TRACE_ENTRIES) / TRACE_ENTRIES_PER_FUNDED_PAIR;
    assert_eq!(
        max_funded, 20,
        "validate.rs quotes 20 rent-exempt pre-funded pairs"
    );

    let (fixture, pairs, result) = validate_with_prefunded(max_funded, max_funded, rent);
    result.unwrap();
    assert_all_proven(&fixture, &pairs);

    let (fixture, pairs, result) = validate_with_prefunded(max_funded + 1, max_funded + 1, rent);
    assert_trace_exceeded(result);
    assert_none_proven(&fixture, &pairs);
}

/// litesvm enforces neither the 1232-byte packet limit nor account locks, so
/// this is the only deliverability check the suite has: the canonical legacy
/// `validate` transaction (1 signature, 10 fixed keys including ComputeBudget,
/// then 33 bytes per pair) is 450 + 33N bytes. 23 pairs fit at 1209; 24 — the
/// batch `MAX_INTENTS_PER_PROVE` tells EVM `Inbox.prove` to send — is 1242 and
/// needs a v0 transaction with an address lookup table. A new fixed account in
/// `Validate` moves this ceiling and the doc sites (validate.rs, CLAUDE.md,
/// spec section 3.4) must move with it. The last assertion is that doc pin,
/// not a size check.
#[test]
#[allow(clippy::assertions_on_constants)]
fn validate_legacy_transaction_fits_only_23_pairs() {
    let authority = Pubkey::new_unique();
    let transaction_len = |pairs: usize| {
        let pairs = rand_pairs(pairs);
        let message = PolymerProver::validate_message(
            &authority,
            PolymerProver::validate_accounts(&authority),
            proof_metas(&pairs),
        );
        // compact-u16 signature count, one 64-byte signature, then the message.
        1 + 64 + message.serialize().len()
    };

    for pairs in [MAX_PAIRS_PER_VALIDATE_LEGACY_TX, MAX_INTENTS_PER_PROVE] {
        assert_eq!(transaction_len(pairs), 450 + 33 * pairs);
    }
    assert!(transaction_len(MAX_PAIRS_PER_VALIDATE_LEGACY_TX) <= PACKET_DATA_SIZE);
    assert!(transaction_len(MAX_PAIRS_PER_VALIDATE_LEGACY_TX + 1) > PACKET_DATA_SIZE);
    // Not a packet-size property: it pins the ALT sentence in validate.rs,
    // CLAUDE.md and spec 3.4. If MAX_INTENTS_PER_PROVE ever drops to the legacy
    // ceiling or below, edit those doc sites, not this test. A runtime assert
    // rather than `const _: () = assert!(..)` so the failure names both numbers.
    assert!(
        MAX_PAIRS_PER_VALIDATE_LEGACY_TX < MAX_INTENTS_PER_PROVE,
        "an inbound batch at the outbound cap ({MAX_INTENTS_PER_PROVE}) needs a v0 transaction \
         with an ALT; a legacy transaction tops out at {MAX_PAIRS_PER_VALIDATE_LEGACY_TX} pairs"
    );
}

/// polymer-prover `validate` → Portal `withdraw`: the Proof `validate` writes is
/// byte-compatible with Portal's strict `Proof::try_from_slice(data[8..])`. The
/// `set_proof`-based tests cannot catch a layout divergence on the polymer side,
/// and a Proof `withdraw` cannot read is unrecoverable — `refund` refuses while a
/// Proof exists.
#[test]
fn validate_withdraw_success() {
    let mut fixture = setup();
    // Destination must be the event's ProofData destination (`event_for` fixes it
    // at EVM_CHAIN_ID); Portal checks `proof.destination == destination`.
    let destination = u64::from(EVM_CHAIN_ID);
    let route_hash: Bytes32 = random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique();
    let reward = Reward {
        deadline: fixture.ctx.now() + 3600,
        creator: fixture.ctx.creator.pubkey(),
        prover: polymer_prover::ID,
        native_amount: 0,
        tokens: vec![],
    };
    let hash = intent_hash(destination, &route_hash, &reward.hash());
    let pairs = vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())];
    let vault = vault_pda(&hash).0;
    fixture.ctx.airdrop(&vault, 1_000_000_000).unwrap();
    let proof = Proof::pda(&hash, &polymer_prover::ID).0;

    load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs)).unwrap();

    // The length Portal's strict decode requires; `set_proof` cannot pin it.
    assert_eq!(
        fixture.ctx.get_account(&proof).unwrap().data.len(),
        8 + ProofAccount::INIT_SPACE
    );

    let payer = fixture.ctx.payer.pubkey();
    let result = fixture.ctx.portal().withdraw_intent(
        destination,
        reward,
        vault,
        route_hash,
        claimant,
        proof,
        WithdrawnMarker::pda(&hash).0,
        proof_closer_pda(&polymer_prover::ID).0,
        vec![],
        iter::once(AccountMeta::new(payer, true)),
    );
    assert!(result.is_ok_and(common::contains_event(IntentWithdrawn::new(hash, claimant))));
    assert!(fixture.ctx.get_account(&proof).is_none());
}

#[test]
fn validate_revalidation_is_idempotent() {
    let mut fixture = setup();
    let pairs = rand_pairs(2);
    load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs)).unwrap();
    let snapshots: Vec<_> = pairs
        .iter()
        .map(|pair| {
            fixture
                .ctx
                .get_account(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
                .unwrap()
        })
        .collect();
    let authority_balance_before = fixture.ctx.balance(&fixture.authority.pubkey());

    // Same proof again: no-op for every pair, event re-emitted.
    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));
    assert!(result.clone().is_ok());
    for pair in &pairs {
        assert!(result
            .clone()
            .is_ok_and(common::contains_cpi_event(prover::IntentProven::new(
                pair.intent_hash,
                Pubkey::new_from_array(pair.claimant.into()),
                EVM_CHAIN_ID.into(),
            ))));
    }

    // A true no-op: every Proof is byte-for-byte what the first call wrote,
    // neither re-created, re-funded nor mutated.
    for (pair, snapshot) in pairs.iter().zip(&snapshots) {
        let after = fixture
            .ctx
            .get_account(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
            .unwrap();
        assert_eq!(after.lamports, snapshot.lamports);
        assert_eq!(after.data, snapshot.data);
        assert_eq!(after.owner, snapshot.owner);
    }
    // And no second rent payment: the spend stays below one Proof's rent-exempt
    // minimum (fees only), whatever number of `load_proof` chunks the reload took.
    assert!(
        authority_balance_before - fixture.ctx.balance(&fixture.authority.pubkey())
            < fixture
                .ctx
                .get_sysvar::<Rent>()
                .minimum_balance(snapshots[0].data.len())
    );
}

/// The stale-result guard. `validate` reads `result_account` after the CPI and
/// trusts that Polymer either overwrote it or aborted (see `validate.rs`). Once
/// a proof has been consumed, a second `validate` without a reload must fail
/// inside `validate_event` rather than replay the previous result.
#[test]
fn validate_twice_without_reload_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs)).unwrap();
    let proof_pda = Proof::pda(&pairs[0].intent_hash, &polymer_prover::ID).0;
    let snapshot = fixture.ctx.get_account(&proof_pda).unwrap();

    // No `polymer_load_result`: the cache was drained by the first validate.
    let result = fixture
        .ctx
        .polymer_prover()
        .validate(&fixture.authority, proof_metas(&pairs));

    // The abort came from inside the Polymer frame, not from one of our gates.
    // The concrete error code is Polymer's (mock: a Borsh decode error), so it
    // is not asserted — only that Polymer did not return success.
    assert!(result.is_err());
    assert!(!result
        .clone()
        .is_err_and(common::program_succeeded(polymer::POLYMER_PROVER_ID)));
    // No stale-result replay and no second rent charge: the first Proof is
    // byte-for-byte untouched.
    let after = fixture.ctx.get_account(&proof_pda).unwrap();
    assert_eq!(after.data, snapshot.data);
    assert_eq!(after.lamports, snapshot.lamports);
}

#[test]
fn validate_disagreeing_claimant_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs)).unwrap();

    let conflicting = vec![IntentHashClaimant::new(
        pairs[0].intent_hash,
        Pubkey::new_unique().to_bytes().into(),
    )];
    let result = load_and_validate(
        &mut fixture,
        &event_for(&conflicting),
        proof_metas(&conflicting),
    );
    assert!(result.is_err_and(common::is_error(PolymerProverError::IntentAlreadyProven)));
}

/// The destination twin of `validate_disagreeing_claimant_fail`: the emitting
/// contract is whitelisted by address alone, so the same address on another EVM
/// chain yields a Polymer-authenticated result that clears the
/// `destination == chain_id` check and must not rewrite the recorded proof.
#[test]
fn validate_disagreeing_destination_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs)).unwrap();

    // Same hash and same claimant, proven from a different EVM chain.
    let event = intent_fulfilled_result(
        EMITTER,
        CHAIN_ID,
        10,
        ProofData::new(10, pairs.clone()).to_bytes(),
    );
    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::IntentAlreadyProven)));
}

#[test]
fn validate_batch_with_one_already_proven_succeeds() {
    let mut fixture = setup();
    let first = rand_pairs(1);
    load_and_validate(&mut fixture, &event_for(&first), proof_metas(&first)).unwrap();

    let mut batch = first.clone();
    batch.extend(rand_pairs(2));
    let result = load_and_validate(&mut fixture, &event_for(&batch), proof_metas(&batch));
    assert!(result.is_ok());
    for pair in &batch {
        assert!(fixture
            .ctx
            .account::<ProofAccount>(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
            .is_some());
    }
}

/// The same intent hash twice **in one event**. `validate` does not dedupe
/// `intent_hashes_claimants` and the relayer supplies one Proof account per
/// pair, so the runtime hands `mark_intent_hash_proven` the same aliased
/// `AccountInfo` twice: entry 1 initialises, entry 2 takes the no-op branch.
/// That relies on duplicate accounts within one instruction sharing a data
/// buffer, so entry 1's write is visible to entry 2's read — otherwise entry 2
/// hits `ConstraintZero` in `create_account` and reverts the whole event.
/// Load-bearing and otherwise unpinned; the `handle` twin is
/// `handle_repeated_intent_hash_same_claimant_success`.
#[test]
fn validate_repeated_intent_hash_same_claimant_success() {
    let mut fixture = setup();
    let pair = rand_pairs(1).remove(0);
    let pairs = vec![pair.clone(), pair.clone()];
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&fixture.authority, &event_for(&pairs))
        .unwrap();
    // Snapshot after the load transactions so the delta covers only `validate`.
    let authority_balance_before = fixture.ctx.balance(&fixture.authority.pubkey());

    let result = fixture
        .ctx
        .polymer_prover()
        .validate(&fixture.authority, proof_metas(&pairs));

    let claimant = Pubkey::new_from_array(pair.claimant.into());
    // Both entries emit, so a consumer counting events rather than upserting by
    // intent hash would double-count.
    assert_eq!(
        result.map(common::count_cpi_events(prover::IntentProven::new(
            pair.intent_hash,
            claimant,
            EVM_CHAIN_ID.into(),
        ))),
        Ok(2)
    );

    let proof_pda = Proof::pda(&pair.intent_hash, &polymer_prover::ID).0;
    let proof: ProofAccount = fixture.ctx.account(&proof_pda).unwrap();
    assert_eq!(proof.0.destination, u64::from(EVM_CHAIN_ID));
    assert_eq!(proof.0.claimant, claimant);
    // One Proof's rent, not two: entry 2 must not re-fund the account.
    assert_eq!(
        authority_balance_before - fixture.ctx.balance(&fixture.authority.pubkey()),
        fixture.ctx.balance(&proof_pda) + TRANSACTION_FEE
    );
}

/// The intra-event twin of `validate_disagreeing_claimant_fail`, mirroring
/// `handle_repeated_intent_hash_other_claimant_fail`. EVM `Inbox.prove` reads
/// the claimant from its fulfilled mapping, so both entries carry the same one
/// today — but that is a property of the caller, not of this guard, and being
/// wrong here means a wrong recorded claimant, not a duplicate log line.
#[test]
fn validate_repeated_intent_hash_other_claimant_fail() {
    let mut fixture = setup();
    let pair = rand_pairs(1).remove(0);
    let pairs = vec![
        pair.clone(),
        IntentHashClaimant::new(pair.intent_hash, Pubkey::new_unique().to_bytes().into()),
    ];

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));

    assert!(result.is_err_and(common::is_error(PolymerProverError::IntentAlreadyProven)));
    // The whole event reverts: entry 1's Proof does not survive.
    assert!(fixture
        .ctx
        .account::<ProofAccount>(&Proof::pda(&pair.intent_hash, &polymer_prover::ID).0)
        .is_none());
}

/// The mock stands in for Polymer's *devnet* deployment, which is what
/// non-mainnet polymer-prover builds CPI into. Under `--features mainnet`
/// polymer-prover targets `MAINNET_POLYMER_PROVER_ID` and the mock-driven
/// tests do not apply; that leg runs only `--test validate_polymer_prover_real`,
/// which loads Polymer's dumped binary over the mock.
#[test]
fn mock_polymer_prover_is_at_polymers_devnet_id() {
    assert_eq!(mock_polymer_prover::ID, polymer::DEVNET_POLYMER_PROVER_ID);
}

/// The mock's `load_proof` → `validate_event` roundtrip: the cache accumulates
/// the loaded body, `validate_event` decodes it into the result account and
/// clears the cache. Serialized bytes are compared so the mock's
/// `ValidationResultAccount` stays a byte-for-byte mirror without a `PartialEq`.
#[test]
fn mock_polymer_load_result_roundtrip() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let event = event_for(&pairs);
    let body = borsh::to_vec(&event).unwrap();

    // load_proof only accumulates into the cache.
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&fixture.authority, &event)
        .unwrap();
    let cache_pda = polymer::cache_pda(&fixture.authority.pubkey()).0;
    let cache = fixture
        .ctx
        .account::<mock_polymer_prover::ProofCacheAccount>(&cache_pda)
        .unwrap();
    assert_eq!(cache.cache, body);

    // validate_event (reached through our `validate`) decodes the cache into
    // the result account and clears it.
    fixture
        .ctx
        .polymer_prover()
        .validate(&fixture.authority, proof_metas(&pairs))
        .unwrap();
    let stored = fixture
        .ctx
        .account::<ValidationResultAccount>(&polymer::result_pda(&fixture.authority.pubkey()).0)
        .unwrap();
    assert_eq!(borsh::to_vec(&stored).unwrap(), body);
    assert!(fixture
        .ctx
        .account::<mock_polymer_prover::ProofCacheAccount>(&cache_pda)
        .unwrap()
        .cache
        .is_empty());
}

#[test]
fn validate_polymer_invalid_result_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    let error_message = "invalid membership proof: can't read path";
    event.is_valid = false;
    event.error_message = error_message.into();

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result
        .clone()
        .is_err_and(common::is_error(PolymerProverError::PolymerProofInvalid)));
    // Spec 3.4 step 2: Polymer's own reason is the relayer's sole diagnostic,
    // so pin the `msg!` and not just the opaque custom error code.
    let logs = result.unwrap_err().meta.logs;
    let expected = format!("Program log: polymer: {error_message}");
    assert!(logs.contains(&expected), "logs: {logs:#?}");
}

#[test]
fn validate_non_whitelisted_emitter_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.emitting_contract = [0x11; 20];

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    // The Polymer frame returned Ok: the rejection is our whitelist gate, not a
    // failure inside `validate_event`. Also keeps `program_succeeded` exercised
    // on every PR, not only in the `#[ignore]`d real-program test.
    assert!(
        result
            .clone()
            .is_err_and(common::program_succeeded(polymer::POLYMER_PROVER_ID)),
        "polymer frame did not succeed: {result:?}"
    );
    assert!(result.is_err_and(common::is_error(
        PolymerProverError::InvalidEmittingContract
    )));
}

#[test]
fn validate_wrong_topics_length_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.topics.extend_from_slice(&[0u8; 32]);

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidTopicsLength)));
}

#[test]
fn validate_wrong_event_signature_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.topics[0] ^= 0xff;

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidEventSignature)));
}

#[test]
fn validate_wrong_source_chain_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let event = intent_fulfilled_result(
        EMITTER,
        CHAIN_ID + 1,
        EVM_CHAIN_ID,
        ProofData::new(EVM_CHAIN_ID.into(), pairs.clone()).to_bytes(),
    );

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidSourceChain)));
}

/// Pins the check order of spec section 3.4 (topics in step 3, payload in step
/// 4): an event that is wrong on both counts reports the chain, not the payload.
/// The two errors mean different things to a relayer (wrong chain vs. corrupt
/// payload), so the order is part of the contract.
#[test]
fn validate_wrong_source_chain_with_malformed_payload_reports_source_chain() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = intent_fulfilled_result(
        EMITTER,
        CHAIN_ID + 1,
        EVM_CHAIN_ID,
        ProofData::new(EVM_CHAIN_ID.into(), pairs.clone()).to_bytes(),
    );
    // ABI offset word 64 instead of 32: `InvalidEventData` on its own.
    event.unindexed_data[31] = 64;

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidSourceChain)));
}

#[test]
fn validate_destination_mismatch_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    // Payload claims destination 10, Polymer says the event came from 8453.
    let event = intent_fulfilled_result(
        EMITTER,
        CHAIN_ID,
        EVM_CHAIN_ID,
        ProofData::new(10, pairs.clone()).to_bytes(),
    );

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(
        PolymerProverError::InvalidDestinationChain
    )));
}

#[test]
fn validate_malformed_abi_data_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.unindexed_data.truncate(40);

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidEventData)));
}

#[test]
fn validate_unaligned_pairs_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut encoded = ProofData::new(EVM_CHAIN_ID.into(), pairs.clone()).to_bytes();
    encoded.push(0);
    let event = intent_fulfilled_result(EMITTER, CHAIN_ID, EVM_CHAIN_ID, encoded);

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidEventData)));
}

#[test]
fn validate_empty_pairs_fail() {
    let mut fixture = setup();
    let event = intent_fulfilled_result(
        EMITTER,
        CHAIN_ID,
        EVM_CHAIN_ID,
        ProofData::new(EVM_CHAIN_ID.into(), vec![]).to_bytes(),
    );

    let result = load_and_validate(&mut fixture, &event, vec![]);
    assert!(result.is_err_and(common::is_error(PolymerProverError::EmptyProofData)));
}

/// Both directions of the arity guard. `==`, not `>=`: there is no
/// partial-batch mode, so a batch whose account list does not line up
/// with the event's pairs is rejected rather than silently truncated.
#[test]
fn validate_proof_account_count_mismatch_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(2);

    // Too few: 2 pairs, 1 Proof account.
    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs[..1]));
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidProof)));

    // Too many: every pair's Proof account, plus one well-formed Proof PDA
    // for a pair the event does not carry — arity is the only defect.
    let mut extra = proof_metas(&pairs);
    extra.extend(proof_metas(&rand_pairs(1)));
    let result = load_and_validate(&mut fixture, &event_for(&pairs), extra);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidProof)));
}

#[test]
fn validate_wrong_proof_pda_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let wrong = vec![AccountMeta::new(Pubkey::new_unique(), false)];

    let result = load_and_validate(&mut fixture, &event_for(&pairs), wrong);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidProof)));
}

/// Pins the `address` half of the `polymer_prover_program` attribute: an
/// executable program that is not Polymer's.
#[test]
fn validate_wrong_polymer_program_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut accounts = PolymerProver::validate_accounts(&fixture.authority.pubkey());
    accounts.polymer_prover_program = local_prover::ID;

    let result = load_and_validate_with_accounts(&mut fixture, &pairs, accounts);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidPolymerProver)));
}

/// A non-executable slot trips `executable` before the address check (Anchor
/// emits them in that order), so this pins the `executable` half of the
/// attribute: the account handed to `invoke` must be a program, not a data
/// account at a spoofed address.
#[test]
fn validate_non_executable_polymer_program_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut accounts = PolymerProver::validate_accounts(&fixture.authority.pubkey());
    accounts.polymer_prover_program = Pubkey::new_unique();

    let result = load_and_validate_with_accounts(&mut fixture, &pairs, accounts);
    assert!(result.is_err_and(common::is_error(
        anchor_lang::error::ErrorCode::ConstraintExecutable
    )));
}

/// Pins the `address` constraint on `result_account`, not the decoder: the
/// constraint rejects this in account validation, before the `validate_event`
/// CPI, so `other`'s Polymer accounts need not exist. `InvalidResultAccount` is
/// also the post-CPI decode error, whose owner and discriminator branches are
/// pinned by unit tests in `polymer.rs` (they are unreachable from here).
#[test]
fn validate_result_account_for_other_authority_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let other = Keypair::new();

    // Authority signs, but points at `other`'s result account.
    let mut accounts = PolymerProver::validate_accounts(&fixture.authority.pubkey());
    accounts.result_account = polymer::result_pda(&other.pubkey()).0;

    let result = load_and_validate_with_accounts(&mut fixture, &pairs, accounts);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidResultAccount)));
}

/// Pins our `address` constraint on `cache_account`, which rejects before the
/// CPI. The mock's own `ConstraintSeeds` would also refuse, but only once the
/// CPI is reached; this is what stops a relayer from spending another
/// authority's staged proof.
#[test]
fn validate_cache_account_for_other_authority_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let other = Keypair::new();
    fixture
        .ctx
        .polymer_prover()
        .polymer_create_accounts(&other)
        .unwrap();
    // `other` has a proof staged in its cache; authority must not be able to
    // spend it.
    fixture
        .ctx
        .polymer_prover()
        .polymer_load_result(&other, &event_for(&pairs))
        .unwrap();

    let mut accounts = PolymerProver::validate_accounts(&fixture.authority.pubkey());
    accounts.cache_account = polymer::cache_pda(&other.pubkey()).0;

    let result = load_and_validate_with_accounts(&mut fixture, &pairs, accounts);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidCacheAccount)));
}

/// Pins our `address` constraint on `internal`, which rejects before the CPI;
/// Polymer's own seeds check on that slot only runs inside `validate_event`.
#[test]
fn validate_wrong_internal_account_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut accounts = PolymerProver::validate_accounts(&fixture.authority.pubkey());
    accounts.internal = Pubkey::new_unique();

    let result = load_and_validate_with_accounts(&mut fixture, &pairs, accounts);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidInternalAccount)));
}

/// Simulates a state no program path can reach — a second `Config` under this
/// program at a non-canonical address — to pin the `address` constraint on
/// `config`. A bare wrong pubkey would trip Anchor's owner/discriminator checks
/// first and never reach that constraint.
#[test]
fn validate_non_canonical_config_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let stray = Pubkey::new_unique();
    let mut data = Config::DISCRIMINATOR.to_vec();
    Config::new(vec![evm_address_to_bytes32(EMITTER)])
        .unwrap()
        .serialize(&mut data)
        .unwrap();
    let lamports = fixture.ctx.get_sysvar::<Rent>().minimum_balance(data.len());
    fixture
        .ctx
        .set_account(
            stray,
            solana_sdk::account::Account {
                lamports,
                data,
                owner: polymer_prover::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();

    let mut accounts = PolymerProver::validate_accounts(&fixture.authority.pubkey());
    accounts.config = stray;

    let result = load_and_validate_with_accounts(&mut fixture, &pairs, accounts);
    assert!(result.is_err_and(common::is_error(PolymerProverError::InvalidConfig)));
}
