use anchor_lang::prelude::*;
use eco_svm_std::account::AccountExt;
use eco_svm_std::prover::Proof;
use eco_svm_std::Bytes32;
use tiny_keccak::{Hasher, Keccak};

use crate::constants::MAX_INTENTS_PER_PROVE;
use crate::instructions::LayerZeroProverError;
use crate::layerzero::LZ_RECEIVE_TYPES_SEED;

pub const STORE_SEED: &[u8] = b"Store";
pub const PDA_PAYER_SEED: &[u8] = b"pda_payer";
pub const PENDING_SEND_SEED: &[u8] = b"pending_send";
pub const MAX_PEERS: usize = 16;
pub const MAX_PAYLOAD_LEN: usize = 8 + 64 * MAX_INTENTS_PER_PROVE;

/// A remote EVM chain this OApp talks to. Also the inbound whitelist and the
/// trusted `src_eid → chain_id` map.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    /// LayerZero endpoint ID of the EVM chain.
    pub eid: u32,
    /// That chain's EVM `LayerZeroProver`, left-padded to 32 bytes.
    pub address: Bytes32,
    /// The chain's EVM chain ID: the only `Proof.destination` it may claim.
    pub chain_id: u64,
}

/// The OApp address registered with the endpoint; signs `send` and `clear`.
#[account]
#[derive(InitSpace, Debug)]
pub struct Store {
    #[max_len(MAX_PEERS)]
    pub peers: Vec<Peer>,
    /// Lookup table the executor uses for `lz_receive`'s static accounts.
    pub alt: Pubkey,
}

impl Store {
    pub fn new(peers: Vec<Peer>) -> Result<Self> {
        require!(
            !peers.is_empty() && peers.len() <= MAX_PEERS,
            LayerZeroProverError::InvalidPeerSet
        );
        peers.iter().enumerate().try_for_each(|(i, peer)| {
            require!(
                peer.eid != 0 && peer.chain_id != 0 && *peer.address != [0u8; 32],
                LayerZeroProverError::InvalidPeerSet
            );
            require!(
                peers[..i]
                    .iter()
                    .all(|other| other.eid != peer.eid && other.chain_id != peer.chain_id),
                LayerZeroProverError::InvalidPeerSet
            );

            Ok(())
        })?;

        Ok(Self {
            peers,
            alt: Pubkey::default(),
        })
    }

    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[STORE_SEED], &crate::ID)
    }

    pub fn peer(&self, eid: u32) -> Option<&Peer> {
        self.peers.iter().find(|peer| peer.eid == eid)
    }
}

impl AccountExt for Store {}

/// Required by the executor at fixed seeds `[LzReceiveTypes, store]`.
#[account]
#[derive(InitSpace, Debug)]
pub struct LzReceiveTypesAccount {
    pub store: Pubkey,
}

impl LzReceiveTypesAccount {
    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[LZ_RECEIVE_TYPES_SEED, Store::pda().0.as_ref()],
            &crate::ID,
        )
    }
}

impl AccountExt for LzReceiveTypesAccount {}

/// Outbound commit written by `prove` (inside `portal::prove`) and consumed by
/// the top-level `send_message`. Its address commits to the content, so an
/// existing account at the derived address is necessarily the same batch.
#[account]
#[derive(InitSpace, Debug, PartialEq)]
pub struct PendingSend {
    pub dst_eid: u32,
    pub receiver: Bytes32,
    #[max_len(MAX_PAYLOAD_LEN)]
    pub payload: Vec<u8>,
    pub rent_payer: Pubkey,
}

impl PendingSend {
    pub fn key(dst_eid: u32, receiver: &Bytes32, payload: &[u8]) -> [u8; 32] {
        let mut hasher = Keccak::v256();
        hasher.update(&dst_eid.to_le_bytes());
        hasher.update(receiver.as_ref());
        hasher.update(payload);
        let mut key = [0u8; 32];
        hasher.finalize(&mut key);

        key
    }

    pub fn pda(dst_eid: u32, receiver: &Bytes32, payload: &[u8]) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[PENDING_SEND_SEED, &Self::key(dst_eid, receiver, payload)],
            &crate::ID,
        )
    }

    pub fn intent_count(&self) -> usize {
        (self.payload.len() - 8) / 64
    }
}

impl AccountExt for PendingSend {}

#[account]
#[derive(InitSpace)]
pub struct ProofAccount(pub Proof);

impl AccountExt for ProofAccount {}

impl From<Proof> for ProofAccount {
    fn from(proof: Proof) -> Self {
        Self(proof)
    }
}

/// System-owned lamport reserve: pays `Proof` rent in `lz_receive` (refunded
/// by `close_proof`) and is the OApp's LayerZero delegate, which the endpoint
/// requires to sign *and* pay for path setup.
pub fn pda_payer_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[PDA_PAYER_SEED], &crate::ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(eid: u32, chain_id: u64, byte: u8) -> Peer {
        Peer {
            eid,
            address: [byte; 32].into(),
            chain_id,
        }
    }

    #[test]
    fn pdas_deterministic() {
        goldie::assert_json!(vec![
            Store::pda(),
            LzReceiveTypesAccount::pda(),
            pda_payer_pda(),
            PendingSend::pda(30184, &[1; 32].into(), &[2; 72]),
        ]);
    }

    /// The portal authority `prove` accepts, scoped to this program's ID. A seed
    /// change silently locks out portal — pin the address. `close_proof`'s
    /// closer is intent-scoped (`proof_closer_pda(intent_hash)`), so it has no
    /// single address to pin here.
    #[test]
    fn accepted_prove_caller_authority_deterministic() {
        goldie::assert_json!(portal::state::dispatcher_pda(&crate::ID));
    }

    #[test]
    fn store_new_accepts_valid_peers() {
        let peers = vec![peer(30184, 8453, 1), peer(30111, 10, 2)];
        assert_eq!(Store::new(peers.clone()).unwrap().peers, peers);
    }

    #[test]
    fn store_new_rejects_invalid_peer_sets() {
        let cases = [
            vec![],
            vec![peer(0, 8453, 1)],
            vec![peer(30184, 0, 1)],
            vec![peer(30184, 8453, 0)],
            vec![peer(30184, 8453, 1), peer(30184, 10, 2)],
            vec![peer(30184, 8453, 1), peer(30111, 8453, 2)],
            (1..=MAX_PEERS as u32 + 1)
                .map(|i| peer(i, i as u64, 1))
                .collect(),
        ];
        cases.into_iter().for_each(|peers| {
            assert!(Store::new(peers).is_err());
        });
    }

    #[test]
    fn pending_send_key_commits_to_every_field() {
        let receiver: Bytes32 = [1; 32].into();
        let base = PendingSend::key(30184, &receiver, &[2; 72]);
        assert_ne!(base, PendingSend::key(30111, &receiver, &[2; 72]));
        assert_ne!(base, PendingSend::key(30184, &[3; 32].into(), &[2; 72]));
        assert_ne!(base, PendingSend::key(30184, &receiver, &[4; 72]));
    }
}
