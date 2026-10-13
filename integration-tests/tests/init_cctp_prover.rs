use cctp_prover::instructions::CctpProverError;
use cctp_prover::state::{Config, MAX_PEERS};
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::cctp_prover_context::{evm_peer, peers, BASE_DOMAIN};

pub mod common;

fn installed() -> (common::Context, Keypair) {
    let mut context = common::Context::default();
    let authority = Keypair::new();
    context.cctp_prover().install(authority.pubkey());
    (context, authority)
}

#[test]
fn init_records_peers_including_ethereum_domain_zero() {
    let (mut context, authority) = installed();

    context.cctp_prover().init(&authority, peers()).unwrap();

    let config = context.account::<Config>(&Config::pda().0).unwrap();
    assert_eq!(config.peers, peers());
    assert_eq!(config.peer(0).unwrap().chain_id, 1);
}

#[test]
fn init_rejects_non_authority_and_repeat() {
    let (mut context, authority) = installed();
    let impostor = Keypair::new();

    let result = context.cctp_prover().init(&impostor, peers());
    assert!(result.is_err_and(common::is_error(CctpProverError::InvalidAuthority)));

    context.cctp_prover().init(&authority, peers()).unwrap();
    context.expire_blockhash();
    assert!(context.cctp_prover().init(&authority, peers()).is_err());
}

#[test]
fn init_rejects_after_finalization() {
    let (mut context, authority) = installed();
    context.cctp_prover().finalize();

    let result = context.cctp_prover().init(&authority, peers());

    assert!(result.is_err_and(common::is_error(CctpProverError::InvalidAuthority)));
}

#[test]
fn init_rejects_invalid_peer_sets() {
    let (mut context, authority) = installed();
    let duplicate = vec![evm_peer(BASE_DOMAIN, 8453, 1), evm_peer(BASE_DOMAIN, 10, 2)];
    let solana = vec![evm_peer(cctp_prover::cctp::SOLANA_DOMAIN, 8453, 1)];

    for invalid in [duplicate, solana, vec![]] {
        let result = context.cctp_prover().init(&authority, invalid);
        assert!(result.is_err_and(common::is_error(CctpProverError::InvalidPeerSet)));
    }
}

/// A full set lands in one `init` (a v1 transaction on a real cluster; see
/// `cctp_prover_batch_limits::max_peers_init_needs_v1`).
#[test]
fn init_accepts_max_peers() {
    let (mut context, authority) = installed();
    let all: Vec<_> = (0..MAX_PEERS)
        .map(|i| evm_peer(100 + i as u32, 900_000 + i as u64, i as u8 + 1))
        .collect();

    context.cctp_prover().init(&authority, all.clone()).unwrap();

    assert_eq!(
        context.account::<Config>(&Config::pda().0).unwrap().peers,
        all
    );
}
