//! Our hand-mirrored endpoint CPIs against LayerZero's real EndpointV2 binary.
//! `#[ignore]`: needs `LZ_ENDPOINT_SO` (see fixtures/layerzero/README.md).

use anchor_lang::AccountSerialize;
use eco_svm_std::prover::{IntentHashClaimant, Proof, ProofData};
use layerzero_prover::layerzero::{self, ENDPOINT_ID};
use layerzero_prover::state::{ProofAccount, Store};
use mock_layerzero_endpoint::{
    EndpointSettings, MessageLibInfo, MessageLibType, Nonce, PayloadHash,
};
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

#[test]
#[ignore]
fn init_path_and_clear_against_real_endpoint() {
    let path = std::env::var("LZ_ENDPOINT_SO").expect("set LZ_ENDPOINT_SO");
    let binary = std::fs::read(path).unwrap();
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.layerzero_prover().install(authority.pubkey());
    context.add_program(ENDPOINT_ID, &binary).unwrap();

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
    let store = Store::pda().0;
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
