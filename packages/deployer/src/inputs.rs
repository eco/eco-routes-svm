use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::str::FromStr;

use anchor_lang::prelude::Pubkey;
use cctp_prover::cctp::SOLANA_DOMAIN;
use eco_svm_std::Bytes32;
use layerzero_prover::instructions::PathConfig;
use layerzero_prover::layerzero::{ExecutorConfig, UlnConfig};
use layerzero_prover::state::{Peer, Store, MAX_PEERS};
use serde::{de, Deserialize, Deserializer, Serialize};

use crate::plan::{
    CCTP_PROVER, HYPER_PROVER, LAYERZERO_PROVER, POLYMER_PROVER, PROGRAMS_WITH_INIT,
};

const EVM_ADDRESS_LEN: usize = 20;
/// eco-svm-std's mainnet and devnet `CHAIN_ID`. The deployer is one build for both clusters, so
/// a CCTP peer may claim neither.
pub const SOLANA_CHAIN_IDS: [u64; 2] = [1399811149, 1399811150];
/// ULN302's `DVN_MAX_LEN`, for the required and the optional list alike.
pub const MAX_DVNS: u8 = 16;

const HYPER_SENDERS: &str = "hyper_senders";
const POLYMER_EMITTERS: &str = "polymer_emitters";
const LAYERZERO: &str = "layerzero";
const CCTP: &str = "cctp";
const HYPER_RESERVE_LAMPORTS: &str = "hyper_reserve_lamports";
const LAYERZERO_RESERVE_LAMPORTS: &str = "layerzero_reserve_lamports";
const COMPUTE_UNIT_PRICE: &str = "compute_unit_price";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{input}: invalid EVM address {value:?}")]
    InvalidEvmAddress { input: &'static str, value: String },
    #[error("{input}: must be a non-empty list without empty entries")]
    Empty { input: &'static str },
    #[error("{input}: duplicate entry {value}")]
    Duplicate { input: &'static str, value: String },
    #[error("invalid layerzero JSON: {reason}")]
    InvalidLayerZeroJson { reason: String },
    #[error("layerzero path for eid {eid} is not fully pinned")]
    UnpinnedPath { eid: u32 },
    #[error("layerzero path for eid {eid} has DVNs that are not strictly ascending")]
    UnsortedDvns { eid: u32 },
    #[error("layerzero path for eid {eid} has more than {MAX_DVNS} required or optional DVNs, which ULN302 rejects")]
    TooManyDvns { eid: u32 },
    #[error("invalid cctp JSON: {reason}")]
    InvalidCctpJson { reason: String },
    #[error("cctp peer for domain {domain}: {reason}")]
    InvalidCctpPeer { domain: u32, reason: &'static str },
    #[error("{input}: {value:?} is not a u64 lamport amount")]
    InvalidLamports { input: &'static str, value: String },
    #[error("{COMPUTE_UNIT_PRICE}: {value:?} is not a u64 micro-lamport price")]
    InvalidComputeUnitPrice { value: String },
    #[error("{input}: the program's init would reject it: {reason}")]
    RejectedByProgram {
        input: &'static str,
        reason: anchor_lang::error::Error,
    },
}

/// The workflow's string inputs, exactly as supplied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawInputs {
    pub hyper_senders: String,
    pub polymer_emitters: String,
    pub layerzero: String,
    pub cctp: String,
    pub hyper_reserve_lamports: String,
    pub layerzero_reserve_lamports: String,
    pub compute_unit_price: String,
}

#[derive(Debug, Clone)]
pub struct Inputs {
    pub hyper_senders: Vec<EvmAddress>,
    pub polymer_emitters: Vec<EvmAddress>,
    pub layerzero_peers: Vec<LayerZeroPeer>,
    pub cctp_peers: Vec<cctp_prover::state::Peer>,
    pub hyper_reserve_lamports: u64,
    pub layerzero_reserve_lamports: u64,
    /// Micro-lamports per compute unit on every deploy and `apply` transaction.
    pub compute_unit_price: u64,
}

#[derive(Debug, Clone)]
pub struct LayerZeroPeer {
    pub eid: u32,
    pub address: EvmAddress,
    pub chain_id: u64,
    pub path: PathConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EvmAddress([u8; EVM_ADDRESS_LEN]);

#[derive(Debug, thiserror::Error)]
#[error("expected 0x followed by 40 hex characters")]
pub struct ParseEvmAddressError;

/// An EVM address as CCTP carries it: 20 bytes left-padded to 32, or the padded 32 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CctpSender(Bytes32);

#[derive(Debug, thiserror::Error)]
pub enum ParseCctpSenderError {
    #[error("expected 0x followed by 40 or 64 hex characters")]
    Malformed,
    #[error("a 32-byte sender must be an EVM address left-padded with 12 zero bytes")]
    NotEvmAddress,
}

impl Inputs {
    pub fn parse(raw: RawInputs) -> Result<Self, Error> {
        Self::parse_selected(
            raw,
            &PROGRAMS_WITH_INIT.into_iter().map(Into::into).collect(),
        )
    }

    pub fn parse_selected(raw: RawInputs, selected: &BTreeSet<String>) -> Result<Self, Error> {
        let RawInputs {
            hyper_senders,
            polymer_emitters,
            layerzero,
            cctp,
            hyper_reserve_lamports,
            layerzero_reserve_lamports,
            compute_unit_price,
        } = raw;

        let hyper_senders = if selected.contains(HYPER_PROVER) {
            parse_address_list(HYPER_SENDERS, &hyper_senders)?
        } else {
            Vec::new()
        };
        if selected.contains(HYPER_PROVER) {
            hyper_prover::state::Config::new(hyper_senders.iter().map(Into::into).collect())
                .map_err(|reason| Error::RejectedByProgram {
                    input: HYPER_SENDERS,
                    reason,
                })?;
        }
        let polymer_emitters = if selected.contains(POLYMER_PROVER) {
            parse_address_list(POLYMER_EMITTERS, &polymer_emitters)?
        } else {
            Vec::new()
        };
        if selected.contains(POLYMER_PROVER) {
            polymer_prover::state::Config::new(polymer_emitters.iter().map(Into::into).collect())
                .map_err(|reason| Error::RejectedByProgram {
                input: POLYMER_EMITTERS,
                reason,
            })?;
        }

        Ok(Self {
            hyper_senders,
            polymer_emitters,
            layerzero_peers: selected
                .contains(LAYERZERO_PROVER)
                .then(|| parse_layerzero_peers(&layerzero))
                .transpose()?
                .unwrap_or_default(),
            cctp_peers: selected
                .contains(CCTP_PROVER)
                .then(|| parse_cctp_peers(&cctp))
                .transpose()?
                .unwrap_or_default(),
            hyper_reserve_lamports: selected
                .contains(HYPER_PROVER)
                .then(|| parse_lamports(HYPER_RESERVE_LAMPORTS, &hyper_reserve_lamports))
                .transpose()?
                .unwrap_or_default(),
            layerzero_reserve_lamports: selected
                .contains(LAYERZERO_PROVER)
                .then(|| parse_lamports(LAYERZERO_RESERVE_LAMPORTS, &layerzero_reserve_lamports))
                .transpose()?
                .unwrap_or_default(),
            compute_unit_price: compute_unit_price.trim().parse().map_err(|_| {
                Error::InvalidComputeUnitPrice {
                    value: compute_unit_price.clone(),
                }
            })?,
        })
    }
}

impl FromStr for EvmAddress {
    type Err = ParseEvmAddressError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let digits = value.strip_prefix("0x").ok_or(ParseEvmAddressError)?;
        let bytes = hex::decode(digits).map_err(|_| ParseEvmAddressError)?;

        bytes.try_into().map(Self).map_err(|_| ParseEvmAddressError)
    }
}

impl fmt::Display for EvmAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "0x{}", hex::encode(self.0))
    }
}

impl FromStr for CctpSender {
    type Err = ParseCctpSenderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let digits = value
            .strip_prefix("0x")
            .ok_or(ParseCctpSenderError::Malformed)?;
        let bytes = hex::decode(digits).map_err(|_| ParseCctpSenderError::Malformed)?;
        let mut padded = [0u8; 32];
        match bytes.len() {
            EVM_ADDRESS_LEN | 32 => padded[32 - bytes.len()..].copy_from_slice(&bytes),
            _ => return Err(ParseCctpSenderError::Malformed),
        }
        // Every peer is an EVM `CCTPProver`; any other sender would be a lane nothing can use.
        if padded[..32 - EVM_ADDRESS_LEN] != [0u8; 32 - EVM_ADDRESS_LEN] {
            return Err(ParseCctpSenderError::NotEvmAddress);
        }

        Ok(Self(padded.into()))
    }
}

impl<'de> Deserialize<'de> for CctpSender {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

impl From<&LayerZeroPeer> for Peer {
    fn from(peer: &LayerZeroPeer) -> Self {
        Self {
            eid: peer.eid,
            address: (&peer.address).into(),
            chain_id: peer.chain_id,
        }
    }
}

impl From<&EvmAddress> for Bytes32 {
    fn from(address: &EvmAddress) -> Self {
        let mut bytes = [0u8; 32];
        bytes[32 - EVM_ADDRESS_LEN..].copy_from_slice(&address.0);

        bytes.into()
    }
}

impl<'de> Deserialize<'de> for EvmAddress {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerZeroInput {
    peers: Vec<PeerInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerInput {
    eid: u32,
    address: EvmAddress,
    chain_id: u64,
    path: PathConfigInput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathConfigInput {
    send_uln: UlnConfigInput,
    receive_uln: UlnConfigInput,
    executor: ExecutorConfigInput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UlnConfigInput {
    confirmations: u64,
    required_dvn_count: u8,
    optional_dvn_count: u8,
    optional_dvn_threshold: u8,
    required_dvns: Vec<Base58Pubkey>,
    optional_dvns: Vec<Base58Pubkey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutorConfigInput {
    max_message_size: u32,
    executor: Base58Pubkey,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CctpInput {
    peers: Vec<CctpPeerInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CctpPeerInput {
    domain: u32,
    chain_id: u64,
    sender: CctpSender,
}

struct Base58Pubkey(Pubkey);

impl<'de> Deserialize<'de> for Base58Pubkey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map(Self)
            .map_err(de::Error::custom)
    }
}

impl From<PathConfigInput> for PathConfig {
    fn from(input: PathConfigInput) -> Self {
        Self {
            send_uln: input.send_uln.into(),
            receive_uln: input.receive_uln.into(),
            executor: input.executor.into(),
        }
    }
}

impl From<UlnConfigInput> for UlnConfig {
    fn from(input: UlnConfigInput) -> Self {
        Self {
            confirmations: input.confirmations,
            required_dvn_count: input.required_dvn_count,
            optional_dvn_count: input.optional_dvn_count,
            optional_dvn_threshold: input.optional_dvn_threshold,
            required_dvns: input.required_dvns.into_iter().map(|dvn| dvn.0).collect(),
            optional_dvns: input.optional_dvns.into_iter().map(|dvn| dvn.0).collect(),
        }
    }
}

impl From<ExecutorConfigInput> for ExecutorConfig {
    fn from(input: ExecutorConfigInput) -> Self {
        Self {
            max_message_size: input.max_message_size,
            executor: input.executor.0,
        }
    }
}

fn parse_address_list(input: &'static str, value: &str) -> Result<Vec<EvmAddress>, Error> {
    let addresses = value
        .split(',')
        .map(str::trim)
        .map(|entry| match entry.is_empty() {
            true => Err(Error::Empty { input }),
            false => entry.parse().map_err(|_| Error::InvalidEvmAddress {
                input,
                value: entry.into(),
            }),
        })
        .collect::<Result<Vec<EvmAddress>, _>>()?;
    reject_duplicates(input, &addresses, ToString::to_string)?;

    Ok(addresses)
}

fn parse_layerzero_peers(value: &str) -> Result<Vec<LayerZeroPeer>, Error> {
    let LayerZeroInput { peers } =
        serde_json::from_str(value).map_err(|error| Error::InvalidLayerZeroJson {
            reason: error.to_string(),
        })?;
    if peers.is_empty() {
        return Err(Error::Empty { input: LAYERZERO });
    }
    if peers.len() > MAX_PEERS {
        return Err(Error::InvalidLayerZeroJson {
            reason: format!("{} peers exceeds the maximum of {MAX_PEERS}", peers.len()),
        });
    }
    reject_duplicates(LAYERZERO, &peers, |peer| peer.eid.to_string())?;

    let peers = peers
        .into_iter()
        .map(|peer| {
            let PeerInput {
                eid,
                address,
                chain_id,
                path,
            } = peer;
            let path: PathConfig = path.into();
            path.validate().map_err(|_| Error::UnpinnedPath { eid })?;
            require_sorted_dvns(eid, &path)?;
            require_dvn_limit(eid, &path)?;

            Ok(LayerZeroPeer {
                eid,
                address,
                chain_id,
                path,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Store::new(peers.iter().map(Into::into).collect()).map_err(|reason| {
        Error::RejectedByProgram {
            input: LAYERZERO,
            reason,
        }
    })?;

    Ok(peers)
}

/// Mirrors `cctp_prover::state::Config::new` with an error per rule, then runs it, so a plan
/// never holds an `init` the program would reject.
fn parse_cctp_peers(value: &str) -> Result<Vec<cctp_prover::state::Peer>, Error> {
    let CctpInput { peers } =
        serde_json::from_str(value).map_err(|error| Error::InvalidCctpJson {
            reason: error.to_string(),
        })?;
    let max_peers = cctp_prover::state::MAX_PEERS;
    if peers.is_empty() {
        return Err(Error::Empty { input: CCTP });
    }
    if peers.len() > max_peers {
        return Err(Error::InvalidCctpJson {
            reason: format!("{} peers exceeds the maximum of {max_peers}", peers.len()),
        });
    }
    reject_duplicates(CCTP, &peers, |peer| format!("domain {}", peer.domain))?;
    reject_duplicates(CCTP, &peers, |peer| format!("chain_id {}", peer.chain_id))?;

    let peers = peers
        .into_iter()
        .map(|peer| {
            let CctpPeerInput {
                domain,
                chain_id,
                sender,
            } = peer;
            let rejection = [
                (domain == SOLANA_DOMAIN, "domain 5 is Solana's own"),
                (chain_id == 0, "chain_id is zero"),
                (
                    SOLANA_CHAIN_IDS.contains(&chain_id),
                    "chain_id is Solana's own",
                ),
                (*sender.0 == [0u8; 32], "sender is zero"),
            ]
            .into_iter()
            .find_map(|(rejected, reason)| rejected.then_some(reason));

            match rejection {
                Some(reason) => Err(Error::InvalidCctpPeer { domain, reason }),
                None => Ok(cctp_prover::state::Peer {
                    domain,
                    chain_id,
                    sender: sender.0,
                }),
            }
        })
        .collect::<Result<Vec<_>, Error>>()?;
    cctp_prover::state::Config::new(peers.clone()).map_err(|reason| Error::RejectedByProgram {
        input: CCTP,
        reason,
    })?;

    Ok(peers)
}

fn require_sorted_dvns(eid: u32, path: &PathConfig) -> Result<(), Error> {
    let strictly_ascending = [&path.send_uln, &path.receive_uln]
        .into_iter()
        .flat_map(|uln| [&uln.required_dvns, &uln.optional_dvns])
        .all(|dvns| dvns.windows(2).all(|pair| pair[0] < pair[1]));

    match strictly_ascending {
        true => Ok(()),
        false => Err(Error::UnsortedDvns { eid }),
    }
}

fn require_dvn_limit(eid: u32, path: &PathConfig) -> Result<(), Error> {
    let within_limit = [&path.send_uln, &path.receive_uln]
        .into_iter()
        .flat_map(|uln| [&uln.required_dvns, &uln.optional_dvns])
        .all(|dvns| dvns.len() <= MAX_DVNS.into());

    match within_limit {
        true => Ok(()),
        false => Err(Error::TooManyDvns { eid }),
    }
}

fn parse_lamports(input: &'static str, value: &str) -> Result<u64, Error> {
    value.trim().parse().map_err(|_| Error::InvalidLamports {
        input,
        value: value.into(),
    })
}

fn reject_duplicates<T>(
    input: &'static str,
    entries: &[T],
    key: impl Fn(&T) -> String,
) -> Result<(), Error> {
    let mut seen = HashSet::new();

    entries
        .iter()
        .map(key)
        .try_for_each(|key| match seen.insert(key.clone()) {
            true => Ok(()),
            false => Err(Error::Duplicate { input, value: key }),
        })
}

#[cfg(test)]
mod tests {
    use std::ops::RangeInclusive;
    use std::slice;

    use super::*;

    const VALID_ADDRESS: &str = "0xAbCdEf0123456789aBcDeF0123456789abcdef01";
    const OTHER_ADDRESS: &str = "0x1111111111111111111111111111111111111111";
    const DVN: &str = "11111111111111111111111111111112";
    const OTHER_DVN: &str = "11111111111111111111111111111114";
    const EXECUTOR: &str = "11111111111111111111111111111113";

    fn uln_json(required_count: u8, optional_count: u8, confirmations: u64) -> String {
        let required = if required_count == 0 || required_count == u8::MAX {
            String::new()
        } else {
            format!("\"{DVN}\"")
        };
        let optional = if optional_count == 0 || optional_count == u8::MAX {
            String::new()
        } else {
            format!("\"{DVN}\"")
        };
        let threshold = if optional_count == u8::MAX || optional_count == 0 {
            0
        } else {
            1
        };

        format!(
            r#"{{"confirmations":{confirmations},"required_dvn_count":{required_count},"optional_dvn_count":{optional_count},"optional_dvn_threshold":{threshold},"required_dvns":[{required}],"optional_dvns":[{optional}]}}"#
        )
    }

    fn peer_json(eid: u32, address: &str, uln: &str) -> String {
        format!(
            r#"{{"eid":{eid},"address":"{address}","chain_id":{eid},"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}"#
        )
    }

    /// `required` and `optional` distinct, ascending DVNs; no optional DVNs is NIL.
    fn uln_with_dvns(required: u8, optional: u8) -> String {
        let dvns = |range: RangeInclusive<u8>| {
            range
                .map(|index| {
                    let mut bytes = [0u8; 32];
                    bytes[31] = index;

                    format!(r#""{}""#, Pubkey::new_from_array(bytes))
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        let (optional_count, threshold) = match optional {
            0 => (u8::MAX, 0),
            count => (count, 1),
        };

        format!(
            r#"{{"confirmations":15,"required_dvn_count":{required},"optional_dvn_count":{optional_count},"optional_dvn_threshold":{threshold},"required_dvns":[{}],"optional_dvns":[{}]}}"#,
            dvns(1..=required),
            dvns(101..=100 + optional)
        )
    }

    fn peer_with_ulns(send: &str, receive: &str) -> String {
        format!(
            r#"{{"eid":30184,"address":"{VALID_ADDRESS}","chain_id":30184,"path":{{"send_uln":{send},"receive_uln":{receive},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}"#
        )
    }

    fn layerzero_json(peers: &[String]) -> String {
        format!(r#"{{"peers":[{}]}}"#, peers.join(","))
    }

    fn valid_layerzero() -> String {
        layerzero_json(&[
            peer_json(30184, VALID_ADDRESS, &uln_json(1, u8::MAX, 15)),
            peer_json(30101, OTHER_ADDRESS, &uln_json(1, 1, 20)),
        ])
    }

    fn addresses(count: u8) -> String {
        (1..=count)
            .map(|index| format!("0x{index:040x}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn rejected_by_store(peers: &[String]) -> bool {
        matches!(
            Inputs::parse(RawInputs {
                layerzero: layerzero_json(peers),
                ..raw_inputs()
            }),
            Err(Error::RejectedByProgram {
                input: "layerzero",
                ..
            })
        )
    }

    fn raw_inputs() -> RawInputs {
        RawInputs {
            hyper_senders: format!("{VALID_ADDRESS}, {OTHER_ADDRESS}"),
            polymer_emitters: OTHER_ADDRESS.into(),
            layerzero: valid_layerzero(),
            cctp: cctp_json(&[
                cctp_peer_json(0, 1, VALID_ADDRESS),
                cctp_peer_json(6, 8453, OTHER_ADDRESS),
            ]),
            hyper_reserve_lamports: "1000000".into(),
            layerzero_reserve_lamports: "2000000".into(),
            compute_unit_price: "0".into(),
        }
    }

    fn cctp_peer_json(domain: u32, chain_id: u64, sender: &str) -> String {
        format!(r#"{{"domain":{domain},"chain_id":{chain_id},"sender":"{sender}"}}"#)
    }

    fn cctp_json(peers: &[String]) -> String {
        format!(r#"{{"peers":[{}]}}"#, peers.join(","))
    }

    fn parse_cctp(peers: &[String]) -> Result<Inputs, Error> {
        Inputs::parse(RawInputs {
            cctp: cctp_json(peers),
            ..raw_inputs()
        })
    }

    #[test]
    fn cctp_accepts_ethereum_domain_zero_and_left_pads_the_sender() {
        let inputs = parse_cctp(&[cctp_peer_json(0, 1, VALID_ADDRESS)]).unwrap();
        let peer = inputs.cctp_peers[0];

        assert_eq!((peer.domain, peer.chain_id), (0, 1));
        let sender: Bytes32 = (&VALID_ADDRESS.parse::<EvmAddress>().unwrap()).into();
        assert_eq!(peer.sender, sender);
    }

    #[test]
    fn cctp_sender_takes_20_or_32_bytes_and_both_forms_agree() {
        let padded = format!("0x{}{}", "00".repeat(12), &VALID_ADDRESS[2..]);
        let short = parse_cctp(&[cctp_peer_json(0, 1, VALID_ADDRESS)]).unwrap();
        let long = parse_cctp(&[cctp_peer_json(0, 1, &padded)]).unwrap();

        assert_eq!(short.cctp_peers, long.cctp_peers);
        [
            format!("0x{}", "ab".repeat(19)),
            format!("0x{}", "ab".repeat(31)),
            format!("0x{}", "ab".repeat(33)),
            "ab".repeat(20),
        ]
        .iter()
        .for_each(|sender| {
            assert!(
                matches!(
                    parse_cctp(&[cctp_peer_json(0, 1, sender)]),
                    Err(Error::InvalidCctpJson { .. })
                ),
                "{sender}"
            );
        });
    }

    #[test]
    fn cctp_rejects_each_invalid_peer() {
        let zero = format!("0x{}", "00".repeat(20));
        let cases = [
            (cctp_peer_json(5, 8453, VALID_ADDRESS), 5),
            (cctp_peer_json(6, 0, VALID_ADDRESS), 6),
            (cctp_peer_json(6, SOLANA_CHAIN_IDS[0], VALID_ADDRESS), 6),
            (cctp_peer_json(6, SOLANA_CHAIN_IDS[1], VALID_ADDRESS), 6),
            (cctp_peer_json(6, 8453, &zero), 6),
        ];

        cases.into_iter().for_each(|(peer, domain)| {
            assert!(
                matches!(
                    parse_cctp(slice::from_ref(&peer)),
                    Err(Error::InvalidCctpPeer { domain: rejected, .. }) if rejected == domain
                ),
                "{peer}"
            );
        });
    }

    #[test]
    fn cctp_rejects_duplicate_domains_and_chain_ids() {
        let duplicate = |peers: &[String], value: &str| {
            matches!(
                parse_cctp(peers),
                Err(Error::Duplicate { input: "cctp", value: duplicate }) if duplicate == value
            )
        };

        assert!(duplicate(
            &[
                cctp_peer_json(6, 8453, VALID_ADDRESS),
                cctp_peer_json(6, 10, OTHER_ADDRESS),
            ],
            "domain 6"
        ));
        assert!(duplicate(
            &[
                cctp_peer_json(6, 8453, VALID_ADDRESS),
                cctp_peer_json(2, 8453, OTHER_ADDRESS),
            ],
            "chain_id 8453"
        ));
    }

    #[test]
    fn cctp_takes_one_to_the_program_maximum_of_peers() {
        let peers = |count: u32| {
            (1..=count)
                .map(|index| cctp_peer_json(100 + index, 900_000 + u64::from(index), VALID_ADDRESS))
                .collect::<Vec<_>>()
        };
        let max_peers = cctp_prover::state::MAX_PEERS as u32;

        assert!(parse_cctp(&peers(max_peers)).is_ok());
        assert!(matches!(
            parse_cctp(&peers(max_peers + 1)),
            Err(Error::InvalidCctpJson { .. })
        ));
        assert!(matches!(
            parse_cctp(&[]),
            Err(Error::Empty { input: "cctp" })
        ));
        assert!(matches!(
            Inputs::parse(RawInputs {
                cctp: "".into(),
                ..raw_inputs()
            }),
            Err(Error::InvalidCctpJson { .. })
        ));
    }

    #[test]
    fn cctp_json_rejects_unknown_fields() {
        let peer = cctp_peer_json(0, 1, VALID_ADDRESS).replacen('{', r#"{"extra":1,"#, 1);

        assert!(matches!(
            parse_cctp(&[peer]),
            Err(Error::InvalidCctpJson { .. })
        ));
    }

    #[test]
    fn cctp_sender_rejects_32_bytes_that_are_not_a_padded_evm_address() {
        let dirty = format!("0x01{}{}", "00".repeat(11), &VALID_ADDRESS[2..]);
        let padded = format!("0x{}{}", "00".repeat(12), &VALID_ADDRESS[2..]);

        assert!(matches!(
            dirty.parse::<CctpSender>(),
            Err(ParseCctpSenderError::NotEvmAddress)
        ));
        assert!(matches!(
            parse_cctp(&[cctp_peer_json(0, 1, &dirty)]),
            Err(Error::InvalidCctpJson { reason }) if reason.contains("left-padded with 12 zero bytes")
        ));
        assert!(parse_cctp(&[cctp_peer_json(0, 1, &padded)]).is_ok());
    }

    /// Both values against eco-svm-std's source, since this build only compiles one of them.
    #[test]
    fn solana_chain_ids_match_eco_svm_std_on_both_clusters() {
        let source = include_str!("../../eco-svm-std/src/lib.rs");
        let [mainnet, devnet] = SOLANA_CHAIN_IDS;

        assert_eq!(mainnet, 1399811149);
        assert_eq!(devnet, 1399811150);
        assert!(source.contains(&format!(
            "#[cfg(feature = \"mainnet\")]\npub const CHAIN_ID: u64 = {mainnet};"
        )));
        assert!(source.contains(&format!(
            "#[cfg(not(feature = \"mainnet\"))]\npub const CHAIN_ID: u64 = {devnet};"
        )));
        assert_eq!(eco_svm_std::CHAIN_ID, devnet);
    }

    #[test]
    fn evm_address_parses_mixed_case_and_displays_lowercase() {
        let address: EvmAddress = VALID_ADDRESS.parse().unwrap();

        assert_eq!(address.to_string(), VALID_ADDRESS.to_lowercase());
    }

    #[test]
    fn evm_address_rejects_wrong_length_and_non_hex() {
        [
            format!("0x{}", "ab".repeat(19)),
            format!("0x{}", "ab".repeat(21)),
            format!("0x{}", "zz".repeat(20)),
            "ab".repeat(20),
        ]
        .iter()
        .for_each(|value| assert!(value.parse::<EvmAddress>().is_err(), "{value}"));
    }

    #[test]
    fn evm_address_left_pads_to_bytes32() {
        let address: EvmAddress = VALID_ADDRESS.parse().unwrap();
        let bytes32: eco_svm_std::Bytes32 = (&address).into();
        let bytes: [u8; 32] = bytes32.into();

        assert_eq!(bytes[..12], [0u8; 12]);
        assert_eq!(hex::encode(&bytes[12..]), VALID_ADDRESS[2..].to_lowercase());
    }

    #[test]
    fn inputs_reject_empty_and_duplicate_lists() {
        let rejected = |raw: RawInputs| Inputs::parse(raw).unwrap_err();

        assert!(matches!(
            rejected(RawInputs {
                hyper_senders: " ".into(),
                ..raw_inputs()
            }),
            Error::Empty {
                input: "hyper_senders"
            }
        ));
        assert!(matches!(
            rejected(RawInputs {
                polymer_emitters: format!("{OTHER_ADDRESS},,{VALID_ADDRESS}"),
                ..raw_inputs()
            }),
            Error::Empty {
                input: "polymer_emitters"
            }
        ));
        assert!(matches!(
            rejected(RawInputs {
                hyper_senders: format!("{VALID_ADDRESS},{}", VALID_ADDRESS.to_lowercase()),
                ..raw_inputs()
            }),
            Error::Duplicate {
                input: "hyper_senders",
                ..
            }
        ));
        assert!(matches!(
            rejected(RawInputs {
                polymer_emitters: format!("{OTHER_ADDRESS},{OTHER_ADDRESS}"),
                ..raw_inputs()
            }),
            Error::Duplicate {
                input: "polymer_emitters",
                ..
            }
        ));
        assert!(matches!(
            rejected(RawInputs {
                layerzero: layerzero_json(&[
                    peer_json(30184, VALID_ADDRESS, &uln_json(1, 1, 15)),
                    peer_json(30184, OTHER_ADDRESS, &uln_json(1, 1, 15)),
                ]),
                ..raw_inputs()
            }),
            Error::Duplicate {
                input: "layerzero",
                ..
            }
        ));
    }

    #[test]
    fn layerzero_rejects_too_many_peers_and_empty_peers() {
        let too_many = (0..=layerzero_prover::state::MAX_PEERS as u32)
            .map(|eid| peer_json(30000 + eid, VALID_ADDRESS, &uln_json(1, 1, 15)))
            .collect::<Vec<_>>();

        assert!(matches!(
            Inputs::parse(RawInputs {
                layerzero: layerzero_json(&too_many),
                ..raw_inputs()
            }),
            Err(Error::InvalidLayerZeroJson { .. })
        ));
        assert!(matches!(
            Inputs::parse(RawInputs {
                layerzero: layerzero_json(&[]),
                ..raw_inputs()
            }),
            Err(Error::Empty { input: "layerzero" })
        ));
    }

    #[test]
    fn layerzero_json_rejects_unknown_fields() {
        let layerzero = valid_layerzero().replacen(r#""eid""#, r#""extra":1,"eid""#, 1);

        assert!(matches!(
            Inputs::parse(RawInputs {
                layerzero,
                ..raw_inputs()
            }),
            Err(Error::InvalidLayerZeroJson { .. })
        ));
    }

    #[test]
    fn layerzero_json_rejects_invalid_base58() {
        let layerzero = valid_layerzero().replace(DVN, "not-base58!");

        assert!(matches!(
            Inputs::parse(RawInputs {
                layerzero,
                ..raw_inputs()
            }),
            Err(Error::InvalidLayerZeroJson { .. })
        ));
    }

    #[test]
    fn layerzero_rejects_default_dvn_counts() {
        [uln_json(0, 1, 15), uln_json(1, 0, 15), uln_json(1, 1, 0)]
            .iter()
            .for_each(|uln| {
                let layerzero = layerzero_json(&[peer_json(30184, VALID_ADDRESS, uln)]);

                assert!(
                    matches!(
                        Inputs::parse(RawInputs {
                            layerzero,
                            ..raw_inputs()
                        }),
                        Err(Error::UnpinnedPath { eid: 30184 })
                    ),
                    "{uln}"
                );
            });
    }

    #[test]
    fn layerzero_rejects_unsorted_and_duplicate_dvns() {
        let with_required = |dvns: [&str; 2]| {
            let uln = uln_json(2, u8::MAX, 15).replace(
                &format!("[\"{DVN}\"]"),
                &format!("[\"{}\",\"{}\"]", dvns[0], dvns[1]),
            );

            layerzero_json(&[peer_json(30184, VALID_ADDRESS, &uln)])
        };
        let rejected = |dvns: [&str; 2]| {
            Inputs::parse(RawInputs {
                layerzero: with_required(dvns),
                ..raw_inputs()
            })
        };

        assert!(matches!(
            rejected([OTHER_DVN, DVN]),
            Err(Error::UnsortedDvns { eid: 30184 })
        ));
        assert!(matches!(
            rejected([DVN, DVN]),
            Err(Error::UnsortedDvns { eid: 30184 })
        ));
        assert!(rejected([DVN, OTHER_DVN]).is_ok());
    }

    #[test]
    fn layerzero_accepts_as_many_dvns_as_uln302() {
        let uln = uln_with_dvns(MAX_DVNS, MAX_DVNS);
        let layerzero = layerzero_json(&[peer_with_ulns(&uln, &uln)]);

        assert!(Inputs::parse(RawInputs {
            layerzero,
            ..raw_inputs()
        })
        .is_ok());
    }

    #[test]
    fn layerzero_rejects_more_dvns_than_uln302() {
        let valid = uln_with_dvns(1, 0);
        [
            uln_with_dvns(MAX_DVNS + 1, 0),
            uln_with_dvns(1, MAX_DVNS + 1),
        ]
        .iter()
        .flat_map(|uln| [peer_with_ulns(uln, &valid), peer_with_ulns(&valid, uln)])
        .for_each(|peer| {
            assert!(
                matches!(
                    Inputs::parse(RawInputs {
                        layerzero: layerzero_json(slice::from_ref(&peer)),
                        ..raw_inputs()
                    }),
                    Err(Error::TooManyDvns { eid: 30184 })
                ),
                "{peer}"
            );
        });
    }

    #[test]
    fn layerzero_accepts_nil_optional_dvns() {
        let layerzero =
            layerzero_json(&[peer_json(30184, VALID_ADDRESS, &uln_json(1, u8::MAX, 15))]);

        assert!(Inputs::parse(RawInputs {
            layerzero,
            ..raw_inputs()
        })
        .is_ok());
    }

    #[test]
    fn compute_unit_price_rejects_non_integer() {
        ["", "-1", "1.5", "abc", "99999999999999999999"]
            .iter()
            .for_each(|value| {
                assert!(matches!(
                    Inputs::parse(RawInputs {
                        compute_unit_price: (*value).into(),
                        ..raw_inputs()
                    }),
                    Err(Error::InvalidComputeUnitPrice { .. })
                ));
            });
    }

    #[test]
    fn lamports_reject_non_integer() {
        ["1.5", "-1", "abc", "", "1e6"].iter().for_each(|value| {
            assert!(matches!(
                Inputs::parse(RawInputs {
                    hyper_reserve_lamports: (*value).into(),
                    ..raw_inputs()
                }),
                Err(Error::InvalidLamports {
                    input: "hyper_reserve_lamports",
                    ..
                })
            ));
            assert!(matches!(
                Inputs::parse(RawInputs {
                    layerzero_reserve_lamports: (*value).into(),
                    ..raw_inputs()
                }),
                Err(Error::InvalidLamports {
                    input: "layerzero_reserve_lamports",
                    ..
                })
            ));
        });
    }

    #[test]
    fn address_lists_name_the_input_with_a_malformed_entry() {
        assert!(matches!(
            Inputs::parse(RawInputs {
                hyper_senders: format!("{VALID_ADDRESS},0x1234"),
                ..raw_inputs()
            }),
            Err(Error::InvalidEvmAddress {
                input: "hyper_senders",
                ..
            })
        ));
        assert!(matches!(
            Inputs::parse(RawInputs {
                polymer_emitters: "0xzz".into(),
                ..raw_inputs()
            }),
            Err(Error::InvalidEvmAddress {
                input: "polymer_emitters",
                ..
            })
        ));
    }

    #[test]
    fn layerzero_rejects_a_default_executor() {
        let default_executor = Pubkey::default().to_string();
        let defaults = [
            valid_layerzero().replace(EXECUTOR, &default_executor),
            valid_layerzero().replace(r#""max_message_size":10000"#, r#""max_message_size":0"#),
        ];

        defaults.into_iter().for_each(|layerzero| {
            assert!(
                matches!(
                    Inputs::parse(RawInputs {
                        layerzero: layerzero.clone(),
                        ..raw_inputs()
                    }),
                    Err(Error::UnpinnedPath { eid: 30184 })
                ),
                "{layerzero}"
            );
        });
    }

    #[test]
    fn hyper_senders_reject_more_than_the_program_holds() {
        assert!(Inputs::parse(RawInputs {
            hyper_senders: addresses(20),
            ..raw_inputs()
        })
        .is_ok());
        assert!(matches!(
            Inputs::parse(RawInputs {
                hyper_senders: addresses(21),
                ..raw_inputs()
            }),
            Err(Error::RejectedByProgram {
                input: "hyper_senders",
                ..
            })
        ));
    }

    #[test]
    fn polymer_emitters_reject_more_than_the_program_holds() {
        assert!(Inputs::parse(RawInputs {
            polymer_emitters: addresses(20),
            ..raw_inputs()
        })
        .is_ok());
        assert!(matches!(
            Inputs::parse(RawInputs {
                polymer_emitters: addresses(21),
                ..raw_inputs()
            }),
            Err(Error::RejectedByProgram {
                input: "polymer_emitters",
                ..
            })
        ));
    }

    #[test]
    fn layerzero_rejects_a_zero_eid() {
        let peer = peer_json(0, VALID_ADDRESS, &uln_json(1, 1, 15))
            .replace(r#""chain_id":0"#, r#""chain_id":8453"#);

        assert!(rejected_by_store(&[peer]));
    }

    #[test]
    fn layerzero_rejects_a_zero_chain_id() {
        let peer = peer_json(30184, VALID_ADDRESS, &uln_json(1, 1, 15))
            .replace(r#""chain_id":30184"#, r#""chain_id":0"#);

        assert!(rejected_by_store(&[peer]));
    }

    #[test]
    fn layerzero_rejects_a_zero_address() {
        let zero = format!("0x{}", "00".repeat(20));
        let peer = peer_json(30184, &zero, &uln_json(1, 1, 15));

        assert!(rejected_by_store(&[peer]));
    }

    #[test]
    fn layerzero_rejects_duplicate_chain_ids() {
        let peers = [
            peer_json(30184, VALID_ADDRESS, &uln_json(1, 1, 15)),
            peer_json(30101, OTHER_ADDRESS, &uln_json(1, 1, 15))
                .replace(r#""chain_id":30101"#, r#""chain_id":30184"#),
        ];

        assert!(rejected_by_store(&peers));
    }

    #[test]
    fn parses_valid_inputs() {
        goldie::assert_debug!(Inputs::parse(raw_inputs()).unwrap());
    }
}
