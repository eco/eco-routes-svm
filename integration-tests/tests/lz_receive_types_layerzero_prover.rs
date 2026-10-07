use anchor_lang::AnchorDeserialize;
use eco_svm_std::prover::{IntentHashClaimant, ProofData};
use layerzero_prover::instructions::{lz_receive_accounts, LayerZeroProverError};
use layerzero_prover::layerzero::{
    LzInstruction, LzReceiveTypesInfoResult, LzReceiveTypesV2Accounts, LzReceiveTypesV2Result,
    EXECUTION_CONTEXT_VERSION_1, LZ_RECEIVE_TYPES_VERSION,
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
fn info_returns_version_two_and_store() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();

    let result = context
        .layerzero_prover()
        .lz_receive_types_info(receive_params(&peers()[0], 1, proof_data()))
        .unwrap();

    assert_eq!(
        LzReceiveTypesInfoResult::try_from_slice(&result.return_data.data).unwrap(),
        LzReceiveTypesInfoResult {
            version: LZ_RECEIVE_TYPES_VERSION,
            accounts: LzReceiveTypesV2Accounts {
                accounts: vec![Store::pda().0]
            },
        }
    );
}

#[test]
fn v2_returns_alt_and_exact_lz_receive_accounts() {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    let params = receive_params(&peers()[0], 1, proof_data());
    let alt = context.account::<Store>(&Store::pda().0).unwrap().alt;

    let result = context
        .layerzero_prover()
        .lz_receive_types_v2(params.clone())
        .unwrap();

    let returned = LzReceiveTypesV2Result::try_from_slice(&result.return_data.data).unwrap();
    assert_eq!(
        returned,
        LzReceiveTypesV2Result {
            context_version: EXECUTION_CONTEXT_VERSION_1,
            alts: vec![alt],
            instructions: vec![LzInstruction::LzReceive {
                accounts: lz_receive_accounts(&params, &proof_data()),
            }],
        }
    );
    // 13 fixed accounts + one Proof PDA per pair.
    let LzInstruction::LzReceive { accounts } = &returned.instructions[0] else {
        panic!()
    };
    assert_eq!(accounts.len(), 13 + 2);
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
