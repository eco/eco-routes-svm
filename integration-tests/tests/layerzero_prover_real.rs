//! Our hand-mirrored endpoint CPIs against LayerZero's real EndpointV2 binary.
//! `#[ignore]`: needs `LZ_ENDPOINT_SO` (see fixtures/layerzero/README.md).

use anchor_lang::AccountSerialize;
use eco_svm_std::prover::{IntentHashClaimant, Proof, ProofData};
use layerzero_prover::layerzero::{self, ENDPOINT_ID};
use layerzero_prover::state::{pda_payer_pda, ProofAccount, Store};
use mock_layerzero_endpoint::{
    EndpointSettings, MessageLibInfo, MessageLibType, Nonce, OAppRegistry, PayloadHash,
    ReceiveLibraryConfig, SendLibraryConfig,
};
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::account::Account;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{
    anchor_account_data, peers, receive_params, BASE_CHAIN_ID,
};

pub mod common;

fn stage<T: AccountSerialize>(context: &mut common::Context, address: Pubkey, account: &T) {
    let data = anchor_account_data(account);
    context
        .set_account(
            address,
            Account {
                lamports: 1_000_000_000,
                data,
                owner: ENDPOINT_ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
}

/// Guards against `add_program` silently leaving the mock in place: the
/// executable bytes at `ENDPOINT_ID` must be the dumped binary and not the mock.
fn assert_real_endpoint_loaded(context: &common::Context, binary: &[u8]) {
    const MOCK: &[u8] = include_bytes!("../../target/deploy/mock_layerzero_endpoint.so");
    assert!(binary != MOCK, "LZ_ENDPOINT_SO points at the mock");
    let program = context.get_account(&ENDPOINT_ID).unwrap();
    assert!(program.executable);
    let loaded = match bincode::deserialize::<UpgradeableLoaderState>(&program.data) {
        Ok(UpgradeableLoaderState::Program {
            programdata_address,
        }) => {
            let data = context.get_account(&programdata_address).unwrap().data;
            data[UpgradeableLoaderState::size_of_programdata_metadata()..].to_vec()
        }
        _ => program.data,
    };
    assert!(
        loaded.starts_with(binary),
        "loaded program is not the dumped binary"
    );
    assert!(!loaded.starts_with(MOCK), "mock endpoint still loaded");
}

#[test]
#[ignore = "needs LZ_ENDPOINT_SO; see tests/fixtures/layerzero/README.md"]
fn init_path_and_clear_against_real_endpoint() {
    let path = std::env::var("LZ_ENDPOINT_SO").expect("set LZ_ENDPOINT_SO");
    let binary = std::fs::read(path).unwrap();
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.layerzero_prover().install(authority.pubkey());
    context.add_program(ENDPOINT_ID, &binary).unwrap();
    assert_real_endpoint_loaded(&context, &binary);

    // What LayerZero's admin set up on-chain: endpoint settings and ULN302's library record.
    let (settings, settings_bump) = layerzero::endpoint_settings_pda();
    stage(
        &mut context,
        settings,
        &EndpointSettings {
            eid: layerzero::DEVNET_SOLANA_EID,
            bump: settings_bump,
            admin: Pubkey::new_unique(),
            lz_token_mint: None,
        },
    );
    let (uln_settings, uln_bump) = layerzero::uln_settings_pda();
    let (info, info_bump) = layerzero::message_lib_info_pda(&uln_settings);
    stage(
        &mut context,
        info,
        &MessageLibInfo {
            message_lib_type: MessageLibType::SendAndReceive,
            bump: info_bump,
            message_lib_bump: uln_bump,
        },
    );

    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();
    let peer = peers()[0];
    context
        .layerzero_prover()
        .init_path(&authority, &peer)
        .unwrap();

    // The real endpoint's own writes, read back through the mock's byte-identical types.
    let store = Store::pda().0;
    let registry: OAppRegistry = context
        .account(&layerzero::oapp_registry_pda(&store).0)
        .unwrap();
    assert_eq!(registry.delegate, pda_payer_pda().0);
    let nonce_account: Nonce = context
        .account(&layerzero::nonce_pda(&store, peer.eid, &peer.address).0)
        .unwrap();
    assert_eq!(nonce_account.outbound_nonce, 0);
    assert_eq!(nonce_account.inbound_nonce, 0);
    let send_config: SendLibraryConfig = context
        .account(&layerzero::send_library_config_pda(&store, peer.eid).0)
        .unwrap();
    assert_eq!(send_config.message_lib, uln_settings);
    let receive_config: ReceiveLibraryConfig = context
        .account(&layerzero::receive_library_config_pda(&store, peer.eid).0)
        .unwrap();
    assert_eq!(receive_config.message_lib, uln_settings);

    // Stand in for DVN verification of nonce 1.
    let claimant = Pubkey::new_unique();
    let data = ProofData::new(
        BASE_CHAIN_ID,
        vec![IntentHashClaimant::new(
            [1; 32].into(),
            claimant.to_bytes().into(),
        )],
    );
    let params = receive_params(&peer, 1, data);
    let (nonce, nonce_bump) = layerzero::nonce_pda(&store, peer.eid, &peer.address);
    stage(
        &mut context,
        nonce,
        &Nonce {
            bump: nonce_bump,
            outbound_nonce: 0,
            inbound_nonce: 1,
        },
    );
    let mut hasher = tiny_keccak::Keccak::v256();
    tiny_keccak::Hasher::update(&mut hasher, &params.guid);
    tiny_keccak::Hasher::update(&mut hasher, &params.message);
    let mut hash = [0u8; 32];
    tiny_keccak::Hasher::finalize(hasher, &mut hash);
    let (payload_hash, payload_bump) =
        layerzero::payload_hash_pda(&store, peer.eid, &peer.address, 1);
    stage(
        &mut context,
        payload_hash,
        &PayloadHash {
            hash,
            bump: payload_bump,
        },
    );

    let result = context.layerzero_prover().lz_receive(&params);

    assert!(result.is_ok(), "{result:?}");
    assert!(context.get_account(&payload_hash).is_none());
    let proof = context
        .account::<ProofAccount>(&Proof::pda(&[1; 32].into(), &layerzero_prover::ID).0)
        .unwrap();
    assert_eq!(proof.0.claimant, claimant);
}
