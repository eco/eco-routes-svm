use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::Proof;
use eco_svm_std::{Bytes32, CHAIN_ID};

use crate::cctp::SOLANA_DOMAIN;
use crate::instructions::CctpProverError;

pub const CONFIG_SEED: &[u8] = b"config";
/// The PDA `message_transmitter_v2::send_message` requires to sign, derived
/// under the sending program (`seeds::program = sender_program`).
pub const SENDER_AUTHORITY_SEED: &[u8] = b"sender_authority";
/// Fixed at `init` and never extended; a chain beyond it needs a new program
/// ID. Past about 19 peers (21 if payer == authority; 32 peers measured at
/// 1694 B as v0, about 44 B per peer) `init` overflows a v0 transaction and
/// needs a v1 one (pinned by `cctp_prover_batch_limits::max_peers_init_needs_v1`).
pub const MAX_PEERS: usize = 32;

/// A remote EVM chain: the inbound whitelist entry and the trusted
/// `remote_domain → chain_id` binding.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// CCTP domain of the EVM chain. 0 is Ethereum.
    pub domain: u32,
    /// The chain's EVM chain ID: the only `Proof.destination` it may claim.
    pub chain_id: u64,
    /// That chain's EVM `CCTPProver`, left-padded to 32 bytes (CCTP's `sender`).
    pub sender: Bytes32,
}

#[account]
#[derive(InitSpace, Debug)]
pub struct Config {
    #[max_len(MAX_PEERS)]
    pub peers: Vec<Peer>,
}

impl Config {
    pub fn new(peers: Vec<Peer>) -> Result<Self> {
        require!(
            !peers.is_empty() && peers.len() <= MAX_PEERS,
            CctpProverError::InvalidPeerSet
        );
        peers.iter().enumerate().try_for_each(|(i, peer)| {
            // Domain 0 is Ethereum and must stay valid; only our own domain and
            // our own chain ID are excluded.
            require!(
                peer.domain != SOLANA_DOMAIN
                    && peer.chain_id != 0
                    && peer.chain_id != CHAIN_ID
                    && *peer.sender != [0u8; 32],
                CctpProverError::InvalidPeerSet
            );
            require!(
                peers[..i]
                    .iter()
                    .all(|other| other.domain != peer.domain && other.chain_id != peer.chain_id),
                CctpProverError::InvalidPeerSet
            );

            Ok(())
        })?;

        Ok(Self { peers })
    }

    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID)
    }

    pub fn peer(&self, domain: u32) -> Option<&Peer> {
        self.peers.iter().find(|peer| peer.domain == domain)
    }
}

impl AccountExt for Config {}

/// Rent is paid by the relayer that submits `receive_message` and refunded on
/// `close_proof` to the proof's claimant (to the closer for a cancellation).
/// There is no protocol reserve, so a fabricated proof costs only whoever
/// relays it.
#[account]
#[derive(InitSpace)]
pub struct ProofAccount(pub Proof);

impl AccountExt for ProofAccount {}

impl From<Proof> for ProofAccount {
    fn from(proof: Proof) -> Self {
        Self(proof)
    }
}

pub fn sender_authority_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SENDER_AUTHORITY_SEED], &crate::ID)
}

#[cfg(test)]
mod tests {
    use eco_svm_std::CHAIN_ID;

    use super::*;

    fn peer(domain: u32, chain_id: u64, byte: u8) -> Peer {
        Peer {
            domain,
            chain_id,
            sender: [byte; 32].into(),
        }
    }

    #[test]
    fn pdas_deterministic() {
        goldie::assert_json!(vec![Config::pda(), sender_authority_pda()]);
    }

    /// The portal authority `prove` accepts, scoped to this program's ID. A seed
    /// change silently locks out portal — pin the address. `close_proof`'s
    /// closer is intent-scoped, so it has no single address to pin here.
    #[test]
    fn accepted_prove_caller_authority_deterministic() {
        goldie::assert_json!(portal::state::dispatcher_pda(&crate::ID));
    }

    #[test]
    fn config_new_accepts_ethereum_domain_zero() {
        let peers = vec![peer(0, 1, 1), peer(6, 8453, 2)];
        assert_eq!(Config::new(peers.clone()).unwrap().peers, peers);
    }

    #[test]
    fn config_new_rejects_invalid_peer_sets() {
        let cases = [
            vec![],
            vec![peer(SOLANA_DOMAIN, 8453, 1)],
            vec![peer(6, 0, 1)],
            vec![peer(6, CHAIN_ID, 1)],
            vec![peer(6, 8453, 0)],
            vec![peer(6, 8453, 1), peer(6, 10, 2)],
            vec![peer(6, 8453, 1), peer(2, 8453, 2)],
            (0..=MAX_PEERS as u32)
                .map(|i| peer(100 + i, 900_000 + i as u64, 1))
                .collect(),
        ];
        cases.into_iter().for_each(|peers| {
            assert!(Config::new(peers).is_err());
        });
    }

    #[test]
    fn config_peer_finds_by_domain() {
        let config = Config::new(vec![peer(0, 1, 1), peer(6, 8453, 2)]).unwrap();
        assert_eq!(config.peer(0).unwrap().chain_id, 1);
        assert_eq!(config.peer(6).unwrap().chain_id, 8453);
        assert!(config.peer(3).is_none());
    }
}
