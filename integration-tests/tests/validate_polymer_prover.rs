use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::AccountDeserialize;
use eco_svm_std::prover::{self, IntentHashClaimant, Proof, ProofData};
use eco_svm_std::CHAIN_ID;
use mock_polymer_prover::ValidationResultAccount;
use polymer_prover::event::evm_address_to_bytes32;
use polymer_prover::instructions::PolymerProverError;
use polymer_prover::polymer;
use polymer_prover::state::{Config, ProofAccount};
use rand::random;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::polymer_prover_context::intent_fulfilled_result;

pub mod common;

/// Whitelisted EVM PolymerProver address used by every test.
const EMITTER: [u8; 20] = [0xec; 20];
/// EVM chain the fulfilments happened on.
const EVM_CHAIN_ID: u32 = 8453;

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

#[test]
fn validate_success() {
    let mut fixture = setup();
    let pairs = rand_pairs(1);
    let authority_balance_before = fixture.ctx.balance(&fixture.authority.pubkey());

    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));

    let claimant = Pubkey::new_from_array(pairs[0].claimant.into());
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
    // The relayer paid the Proof rent.
    assert!(fixture.ctx.balance(&fixture.authority.pubkey()) < authority_balance_before);
    // Polymer cleared the cache.
    let cache = fixture
        .ctx
        .get_account(&polymer::cache_pda(&fixture.authority.pubkey()).0)
        .unwrap();
    let cache = mock_polymer_prover::ProofCacheAccount::try_deserialize(&mut cache.data.as_slice())
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

    // Same proof again: no-op for every pair, event re-emitted.
    let result = load_and_validate(&mut fixture, &event_for(&pairs), proof_metas(&pairs));
    assert!(result.clone().is_ok());
    assert!(
        result.is_ok_and(common::contains_cpi_event(prover::IntentProven::new(
            pairs[1].intent_hash,
            Pubkey::new_from_array(pairs[1].claimant.into()),
            EVM_CHAIN_ID.into(),
        )))
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

#[test]
fn mock_polymer_load_result_roundtrip() {
    let mut ctx = common::Context::default();
    let authority = Keypair::new();
    ctx.polymer_prover()
        .polymer_create_accounts(&authority)
        .unwrap();

    let result = intent_fulfilled_result([0xab; 20], 7, 8453, vec![1u8; 72]);
    ctx.polymer_prover()
        .polymer_load_result(&authority, &result)
        .unwrap();

    let cache = ctx
        .get_account(&polymer::cache_pda(&authority.pubkey()).0)
        .unwrap();
    let cache = mock_polymer_prover::ProofCacheAccount::try_deserialize(&mut cache.data.as_slice())
        .unwrap();
    assert_eq!(cache.cache, borsh::to_vec(&result).unwrap());
    let _: ValidationResultAccount = ctx
        .account(&polymer::result_pda(&authority.pubkey()).0)
        .unwrap();
}
