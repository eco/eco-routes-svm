use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::{AnchorSerialize, Discriminator};
use eco_svm_std::prover::{self, IntentHashClaimant, Proof, ProofData};
use eco_svm_std::CHAIN_ID;
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::event::evm_address_to_bytes32;
use polymer_prover::instructions::PolymerProverError;
use polymer_prover::polymer;
use polymer_prover::state::{Config, ProofAccount};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::polymer_prover_context::{intent_fulfilled_result, PolymerProver};

pub mod common;

/// Whitelisted EVM PolymerProver address used by every test.
const EMITTER: [u8; 20] = [0xec; 20];
/// EVM chain the fulfilments happened on.
const EVM_CHAIN_ID: u32 = 8453;
/// LiteSVM's default fee structure charges 5000 lamports per signature; the
/// `validate` transaction carries exactly one (the authority's).
const TRANSACTION_FEE: u64 = 5_000;

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

#[test]
fn mock_polymer_prover_is_at_the_id_polymer_prover_targets() {
    assert_eq!(mock_polymer_prover::ID, polymer::POLYMER_PROVER_ID);
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
    event.is_valid = false;
    event.error_message = "invalid membership proof: can't read path".into();

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
    assert!(result.is_err_and(common::is_error(PolymerProverError::PolymerProofInvalid)));
}

#[test]
fn validate_non_whitelisted_emitter_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let mut event = event_for(&pairs);
    event.emitting_contract = [0x11; 20];

    let result = load_and_validate(&mut fixture, &event, proof_metas(&pairs));
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

#[test]
fn validate_proof_account_count_mismatch_fail() {
    let mut fixture = setup();
    let pairs = rand_pairs(2);

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs[..1]));
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
