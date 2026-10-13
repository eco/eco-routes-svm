use anchor_lang::prelude::borsh;
use layerzero_prover::instructions::{required_alt_addresses, LayerZeroProverError, PathConfig};
use layerzero_prover::layerzero::{
    self, CONFIG_TYPE_EXECUTOR, CONFIG_TYPE_RECEIVE_ULN, CONFIG_TYPE_SEND_ULN, NIL_CONFIRMATIONS,
    NIL_DVN_COUNT,
};
use layerzero_prover::state::{pda_payer_pda, Store, MAX_PAYLOAD_LEN, MAX_PEERS};
use mock_layerzero_endpoint::{
    MockConfigInitialized, MockConfigSet, Nonce, OAppRegistry, ReceiveLibraryConfig,
    SendLibraryConfig,
};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{evm_peer, path_config, peers, BASE_EID};

pub mod common;

fn installed() -> (common::Context, Keypair) {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.layerzero_prover().install(authority.pubkey());
    (context, authority)
}

#[test]
fn init_registers_store_with_pda_payer_as_delegate() {
    let (mut context, authority) = installed();

    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();

    let store = context.account::<Store>(&Store::pda().0).unwrap();
    assert_eq!(store.peers, peers());
    assert_eq!(store.alt, Pubkey::default());
    let registry = context
        .account::<OAppRegistry>(&layerzero::oapp_registry_pda(&Store::pda().0).0)
        .unwrap();
    assert_eq!(registry.delegate, pda_payer_pda().0);
}

#[test]
fn init_rejects_non_authority_and_repeat() {
    let (mut context, authority) = installed();
    let impostor = Keypair::new();

    let result = context.layerzero_prover().init(&impostor, peers());
    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidAuthority)));

    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();
    context.expire_blockhash();
    assert!(context
        .layerzero_prover()
        .init(&authority, peers())
        .is_err());
}

#[test]
fn init_rejects_invalid_peer_set() {
    let (mut context, authority) = installed();
    let duplicate = vec![evm_peer(BASE_EID, 8453, 1), evm_peer(BASE_EID, 10, 2)];

    let result = context.layerzero_prover().init(&authority, duplicate);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidPeerSet)));
}

/// A full peer set lands in one `init` (a v1 transaction on a real cluster;
/// see `layerzero_prover_batch_limits::max_peers_init_needs_v1`), and the
/// recorded lookup table covers every peer's Nonce.
#[test]
fn init_accepts_max_peers() {
    let (mut context, authority) = installed();
    let all: Vec<_> = (0..MAX_PEERS)
        .map(|i| evm_peer(50_000 + i as u32, 900_000 + i as u64, i as u8 + 1))
        .collect();

    context
        .layerzero_prover()
        .init(&authority, all.clone())
        .unwrap();

    assert_eq!(
        context.account::<Store>(&Store::pda().0).unwrap().peers,
        all
    );
    let alt = context.layerzero_prover().create_alt();
    context.layerzero_prover().set_alt(&authority, alt).unwrap();
}

#[test]
fn init_path_creates_nonce_and_pins_uln_for_both_directions() {
    let (mut context, authority) = installed();
    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();
    let peer = peers()[0];
    let store = Store::pda().0;

    context
        .layerzero_prover()
        .init_path(&authority, &peer)
        .unwrap();

    let uln = layerzero::uln_settings_pda().0;
    assert!(context
        .account::<Nonce>(&layerzero::nonce_pda(&store, peer.eid, &peer.address).0)
        .is_some());
    let send_library = context
        .account::<SendLibraryConfig>(&layerzero::send_library_config_pda(&store, peer.eid).0)
        .unwrap();
    assert_eq!(send_library.message_lib, uln);
    let receive_library = context
        .account::<ReceiveLibraryConfig>(&layerzero::receive_library_config_pda(&store, peer.eid).0)
        .unwrap();
    assert_eq!(receive_library.message_lib, uln);
}

#[test]
fn init_path_rejects_unknown_peer() {
    let (mut context, authority) = installed();
    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();

    let result = context
        .layerzero_prover()
        .init_path(&authority, &evm_peer(40_245, 84532, 0x11));

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::UnknownPeer)));
}

#[test]
fn set_path_config_pins_send_receive_and_executor() {
    let (mut context, authority) = installed();
    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();
    context
        .layerzero_prover()
        .init_path(&authority, &peers()[0])
        .unwrap();
    let config = path_config();
    let oapp = Store::pda().0;

    let result = context
        .layerzero_prover()
        .set_path_config(&authority, BASE_EID, config.clone())
        .unwrap();

    assert!(common::contains_event(MockConfigInitialized {
        oapp,
        eid: BASE_EID
    })(result.clone()));
    [
        (
            CONFIG_TYPE_SEND_ULN,
            borsh::to_vec(&config.send_uln).unwrap(),
        ),
        (
            CONFIG_TYPE_RECEIVE_ULN,
            borsh::to_vec(&config.receive_uln).unwrap(),
        ),
        (
            CONFIG_TYPE_EXECUTOR,
            borsh::to_vec(&config.executor).unwrap(),
        ),
    ]
    .into_iter()
    .for_each(|(config_type, config)| {
        assert!(common::contains_event(MockConfigSet {
            oapp,
            eid: BASE_EID,
            config_type,
            config
        })(result.clone()));
    });
}

#[test]
fn set_path_config_rejects_anything_left_on_layerzero_defaults() {
    let mutations: [fn(&mut PathConfig); 14] = [
        |c| c.send_uln.confirmations = 0,
        |c| c.receive_uln.confirmations = 0,
        // ULN302 resolves NIL confirmations to an explicit 0.
        |c| c.send_uln.confirmations = NIL_CONFIRMATIONS,
        |c| c.receive_uln.confirmations = NIL_CONFIRMATIONS,
        |c| c.receive_uln.required_dvn_count = 0,
        |c| c.receive_uln.required_dvn_count = NIL_DVN_COUNT,
        |c| {
            c.send_uln.required_dvns.pop();
        },
        |c| c.executor.max_message_size = 0,
        |c| c.executor.max_message_size = MAX_PAYLOAD_LEN as u32 - 1,
        |c| c.executor.executor = Pubkey::default(),
        |c| c.receive_uln.optional_dvn_count = 0,
        |c| c.send_uln.optional_dvns.push(Pubkey::new_unique()),
        |c| {
            c.send_uln.optional_dvn_count = 1;
            c.send_uln.optional_dvns = vec![Pubkey::new_unique()];
        },
        |c| {
            c.receive_uln.optional_dvn_count = 1;
            c.receive_uln.optional_dvn_threshold = 2;
            c.receive_uln.optional_dvns = vec![Pubkey::new_unique()];
        },
    ];
    mutations.into_iter().for_each(|mutate| {
        let (mut context, authority) = installed();
        context
            .layerzero_prover()
            .init(&authority, peers())
            .unwrap();
        context
            .layerzero_prover()
            .init_path(&authority, &peers()[0])
            .unwrap();
        let mut config = path_config();
        mutate(&mut config);

        let result = context
            .layerzero_prover()
            .set_path_config(&authority, BASE_EID, config);

        assert!(result.is_err_and(common::is_error(LayerZeroProverError::UnpinnedConfig)));
    });
}

#[test]
fn set_alt_records_lookup_table() {
    let (mut context, authority) = installed();
    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();
    let alt = context.layerzero_prover().create_alt();

    context.layerzero_prover().set_alt(&authority, alt).unwrap();

    assert_eq!(context.account::<Store>(&Store::pda().0).unwrap().alt, alt);
}

#[test]
fn set_alt_rejects_non_lookup_table_account() {
    let (mut context, authority) = installed();
    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();
    let not_a_table = Pubkey::new_unique();
    context.airdrop(&not_a_table, 1_000_000_000).unwrap();

    let result = context.layerzero_prover().set_alt(&authority, not_a_table);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidLookupTable)));
}

#[test]
fn set_alt_rejects_malformed_lookup_table_data() {
    let (mut context, authority) = initialized();
    let alt = context.layerzero_prover().create_alt();
    let mut account = context.get_account(&alt).unwrap();
    // Uninitialized tag, then a trailing partial address.
    let mut uninitialized = account.clone();
    uninitialized.data[..4].copy_from_slice(&0u32.to_le_bytes());
    account.data.push(0);

    [uninitialized, account].into_iter().for_each(|staged| {
        context.set_account(alt, staged).unwrap();
        context.expire_blockhash();

        let result = context.layerzero_prover().set_alt(&authority, alt);

        assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidLookupTable)));
    });
}

fn initialized() -> (common::Context, Keypair) {
    let (mut context, authority) = installed();
    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();
    (context, authority)
}

fn required_addresses(context: &common::Context) -> Vec<Pubkey> {
    required_alt_addresses(&context.account::<Store>(&Store::pda().0).unwrap())
}

#[test]
fn set_alt_rejects_table_with_authority() {
    let (mut context, authority) = initialized();
    let addresses = required_addresses(&context);
    let alt =
        context
            .layerzero_prover()
            .create_alt_with(Some(authority.pubkey()), u64::MAX, addresses);

    let result = context.layerzero_prover().set_alt(&authority, alt);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::LookupTableNotFrozen)));
}

#[test]
fn set_alt_rejects_deactivated_table() {
    let (mut context, authority) = initialized();
    let addresses = required_addresses(&context);
    let alt = context
        .layerzero_prover()
        .create_alt_with(None, 1, addresses);

    let result = context.layerzero_prover().set_alt(&authority, alt);

    assert!(result.is_err_and(common::is_error(
        LayerZeroProverError::LookupTableDeactivated
    )));
}

#[test]
fn set_alt_rejects_table_missing_a_peer_nonce() {
    let (mut context, authority) = initialized();
    let peer = peers()[1];
    let nonce = layerzero::nonce_pda(&Store::pda().0, peer.eid, &peer.address).0;
    let addresses: Vec<Pubkey> = required_addresses(&context)
        .into_iter()
        .filter(|address| *address != nonce)
        .collect();
    let alt = context
        .layerzero_prover()
        .create_alt_with(None, u64::MAX, addresses);

    let result = context.layerzero_prover().set_alt(&authority, alt);

    assert!(result.is_err_and(common::is_error(
        LayerZeroProverError::LookupTableMissingAddress
    )));
    assert_eq!(
        context.account::<Store>(&Store::pda().0).unwrap().alt,
        Pubkey::default()
    );
}

#[test]
fn finalized_program_rejects_every_setup_instruction() {
    let (mut context, authority) = installed();
    context
        .layerzero_prover()
        .init(&authority, peers())
        .unwrap();
    let alt = context.layerzero_prover().create_alt();
    context.layerzero_prover().finalize();

    let peer = peers()[0];
    let results = [
        context.layerzero_prover().init_path(&authority, &peer),
        context
            .layerzero_prover()
            .set_path_config(&authority, peer.eid, path_config()),
        context.layerzero_prover().set_alt(&authority, alt),
    ];

    results.into_iter().for_each(|result| {
        assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidAuthority)));
    });
}
