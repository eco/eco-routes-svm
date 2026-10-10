use anchor_lang::AnchorDeserialize;
use eco_svm_std::prover::{IntentHashClaimant, Proof, ProofData};
use layerzero_prover::instructions::{lz_receive_accounts, LayerZeroProverError};
use layerzero_prover::layerzero::{
    self, AddressLocator, LzInstruction, LzReceiveTypesInfoResult, LzReceiveTypesV2Accounts,
    LzReceiveTypesV2Result, EXECUTION_CONTEXT_VERSION_1, LZ_RECEIVE_TYPES_VERSION,
};
use layerzero_prover::state::Store;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{peers, receive_params, BASE_CHAIN_ID};

pub mod common;

fn proof_data() -> ProofData {
    ProofData::new(
        BASE_CHAIN_ID,
        vec![
            IntentHashClaimant::new([1; 32].into(), [2; 32].into()),
            IntentHashClaimant::new([3; 32].into(), [4; 32].into()),
        ],
    )
}

#[test]
fn info_returns_version_two_store_and_alt() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let alt = context.account::<Store>(&Store::pda().0).unwrap().alt;

    let result = context
        .layerzero_prover()
        .lz_receive_types_info(receive_params(&peers()[0], 1, proof_data()))
        .unwrap();

    assert_eq!(
        LzReceiveTypesInfoResult::try_from_slice(&result.return_data.data).unwrap(),
        LzReceiveTypesInfoResult {
            version: LZ_RECEIVE_TYPES_VERSION,
            accounts: LzReceiveTypesV2Accounts {
                accounts: vec![Store::pda().0, alt]
            },
        }
    );
}

/// v2 answers with the store's table and `lz_receive_accounts` compacted
/// against it: every static account rides in the table as an `AltIndex`, only
/// the per-message PayloadHash and Proof PDAs stay plain `Address`es, and the
/// whole list resolves back to exactly what `lz_receive` validates.
#[test]
fn v2_returns_alt_and_compacted_lz_receive_accounts() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let params = receive_params(&peers()[0], 1, proof_data());
    let alt = context.account::<Store>(&Store::pda().0).unwrap().alt;

    let result = context
        .layerzero_prover()
        .lz_receive_types_v2(params.clone())
        .unwrap();

    let mut returned = LzReceiveTypesV2Result::try_from_slice(&result.return_data.data).unwrap();
    assert_eq!(returned.context_version, EXECUTION_CONTEXT_VERSION_1);
    assert_eq!(returned.alts, vec![alt]);
    assert_eq!(returned.instructions.len(), 1);
    let LzInstruction::LzReceive { accounts } = returned.instructions.remove(0) else {
        panic!("expected an LzReceive instruction")
    };
    let expected = lz_receive_accounts(&params, &proof_data());
    // 13 fixed accounts + one Proof PDA per pair.
    assert_eq!(accounts.len(), 13 + 2);
    assert_eq!(
        context
            .layerzero_prover()
            .resolve_locators(&returned.alts, accounts.clone()),
        expected
    );
    let payload_hash =
        layerzero::payload_hash_pda(&Store::pda().0, params.src_eid, &params.sender, 1).0;
    let per_message: Vec<AddressLocator> = [payload_hash]
        .into_iter()
        .chain(
            proof_data()
                .intent_hashes_claimants
                .iter()
                .map(|pair| Proof::pda(&pair.intent_hash, &layerzero_prover::ID).0),
        )
        .map(AddressLocator::Address)
        .collect();
    accounts.iter().for_each(|meta| match meta.pubkey {
        AddressLocator::AltIndex(0, _) => {}
        ref address => assert!(per_message.contains(address), "{address:?} not compacted"),
    });
    assert_eq!(
        accounts
            .iter()
            .filter(|meta| matches!(meta.pubkey, AddressLocator::AltIndex(..)))
            .count(),
        13 - 1
    );
}

#[test]
fn v2_rejects_a_table_other_than_the_recorded_one() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let other = context.layerzero_prover().create_alt();

    let result = context
        .layerzero_prover()
        .lz_receive_types_v2_with_alt(receive_params(&peers()[0], 1, proof_data()), other);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidLookupTable)));
}

#[test]
fn v2_requires_alt() {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.layerzero_prover().install(authority.pubkey());
    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();

    let result = context
        .layerzero_prover()
        .lz_receive_types_v2(receive_params(&peers()[0], 1, proof_data()));

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::AltNotSet)));
}

#[test]
fn v2_rejects_malformed_message() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let mut params = receive_params(&peers()[0], 1, proof_data());
    params.message.push(0);

    assert!(context
        .layerzero_prover()
        .lz_receive_types_v2(params)
        .is_err());
}
