use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use eco_svm_std::Bytes32;
use layerzero_prover::instructions::PathConfig;
use layerzero_prover::layerzero::{ExecutorConfig, UlnConfig};
use serde::{Deserialize, Serialize};
use solana_cluster_type::ClusterType;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;

use crate::classify::{ProgramState, Status};
use crate::config::{self, Configs};
use crate::inputs::{EvmAddress, Inputs, LayerZeroPeer, RawInputs};
use crate::layerzero_state::{Alt, Path};
use crate::{layerzero, setup};

pub const HYPER_PROVER: &str = "hyper_prover";
pub const LOCAL_PROVER: &str = "local_prover";
pub const POLYMER_PROVER: &str = "polymer_prover";
pub const AGGREGATOR_PROVER: &str = "aggregator_prover";
pub const LAYERZERO_PROVER: &str = "layerzero_prover";
/// Mirrors `AGGREGATOR_MEMBERS` in `scripts/program-keypairs.mjs`; the order is the on-chain config order.
pub const AGGREGATOR_MEMBERS: [&str; 3] = [HYPER_PROVER, POLYMER_PROVER, LAYERZERO_PROVER];
pub const PROGRAMS_WITH_INIT: [&str; 4] = [
    HYPER_PROVER,
    POLYMER_PROVER,
    AGGREGATOR_PROVER,
    LAYERZERO_PROVER,
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the RPC serves the ledger with genesis hash {actual}, not {cluster} ({expected})")]
    ClusterMismatch {
        cluster: Cluster,
        expected: Hash,
        actual: Hash,
    },
    #[error("{program} is not ours to deploy: {reason}")]
    Foreign { program: String, reason: String },
    #[error("no on-chain state for {program}")]
    MissingState { program: String },
    #[error("{program}: release address {release} differs from classified address {state}")]
    AddressMismatch {
        program: String,
        release: Pubkey,
        state: Pubkey,
    },
    #[error(transparent)]
    ConfigMismatch(#[from] config::Mismatch),
    #[error(transparent)]
    LayerZero(#[from] layerzero::Error),
    #[error("{program} is live (immutable) but has no config, so it can never be initialized; raise its salt in scripts/program-salts.json to redeploy it at a new address")]
    LiveWithoutConfig { program: &'static str },
    #[error("unknown cluster {value:?}, expected devnet or mainnet")]
    UnknownCluster { value: String },
    #[error("invalid plan hash {value:?}, expected 64 hex characters")]
    InvalidPlanHash { value: String },
    #[error("{LAYERZERO_PROVER} is released at {release} but this deployer was built for {compiled}; build it from the release's deploy tag")]
    BuiltForAnotherRelease { release: Pubkey, compiled: Pubkey },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cluster {
    Devnet,
    Mainnet,
}

/// What `plan.json` records: everything `apply` needs to rebuild the plan and recompute its hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDocument {
    pub release: Release,
    pub inputs: RawInputs,
}

/// What `deployer plan` writes: the document `apply` rebuilds from, plus the plan it produced.
/// `actions` reads only this; `apply` compares its live rebuild against `hash` and `programs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanFile {
    #[serde(flatten)]
    pub document: PlanDocument,
    pub genesis_hash: String,
    pub hash: String,
    pub programs: BTreeMap<String, PlannedProgramFile>,
    pub live_configs: BTreeMap<String, Option<Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedProgramFile {
    pub status: PlannedStatus,
    pub authority: Option<String>,
    pub data_hash: Option<String>,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub cluster: Cluster,
    pub programs: BTreeMap<String, ReleaseProgram>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseProgram {
    #[serde(with = "pubkey_string")]
    pub address: Pubkey,
    #[serde(with = "hex_array")]
    pub so_sha256: [u8; 32],
    /// Released programs whose addresses this one's bytecode or config holds, from
    /// `program-ids.json`; a program is finalized only after all of them.
    pub dependencies: Vec<String>,
}

/// A program's status as `plan.json` and the summary name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlannedStatus {
    New,
    Partial,
    Live,
    Foreign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Deploy,
    Init,
    Verify,
    Finalize,
    Close,
}

#[derive(Debug, Clone)]
pub struct PlannedProgram {
    pub state: ProgramState,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub release: Release,
    /// Of the ledger the RPC serves; equal to the release cluster's.
    pub genesis_hash: Hash,
    /// Pays for the run and is every new program's upgrade authority until finalize.
    pub deployer: Pubkey,
    pub programs: BTreeMap<String, PlannedProgram>,
    pub inputs: Inputs,
    /// Prover configs as found on chain at planning time; part of the plan hash.
    pub live_configs: Configs,
    /// Prover configs the inputs and release call for.
    pub expected_configs: Configs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanHash([u8; 32]);

impl Plan {
    pub fn build(
        release: Release,
        genesis_hash: Hash,
        deployer: Pubkey,
        mut states: BTreeMap<String, ProgramState>,
        live_configs: Configs,
        inputs: Inputs,
    ) -> Result<Self, Error> {
        release.cluster.require_genesis(genesis_hash)?;
        require_compiled_layerzero(&release)?;
        let expected_configs = expected_configs(&release, &inputs)?;
        layerzero::check_transaction_sizes(
            &release.programs[LAYERZERO_PROVER].address,
            &inputs.layerzero_peers,
        )?;
        let programs = release
            .programs
            .iter()
            .map(|(name, program)| {
                let state = states.remove(name).ok_or_else(|| Error::MissingState {
                    program: name.clone(),
                })?;
                reject_foreign(name, program, &state)?;

                Ok((name.clone(), state))
            })
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        let programs = programs
            .into_iter()
            .map(|(name, state)| {
                let actions = actions(&name, &state.status);

                (name, PlannedProgram { state, actions })
            })
            .collect();

        let plan = Self {
            release,
            genesis_hash,
            deployer,
            programs,
            inputs,
            live_configs,
            expected_configs,
        };
        plan.verify_configs(&plan.live_configs)?;

        Ok(plan)
    }

    /// Rejects `live` configs that differ from the inputs, and live programs that lack one.
    pub fn verify_configs(&self, live: &Configs) -> Result<(), Error> {
        live.conflict(&self.expected_configs)?;

        let layerzero_incomplete = setup::check(live).is_err();

        live.entries()
            .into_iter()
            .filter(|(program, values)| {
                values.is_none() || (*program == LAYERZERO_PROVER && layerzero_incomplete)
            })
            .try_for_each(|(program, _)| match self.programs.get(program) {
                Some(planned) if planned.state.status == Status::Live => {
                    Err(Error::LiveWithoutConfig { program })
                }
                _ => Ok(()),
            })
    }

    pub fn hash(&self) -> PlanHash {
        PlanHash::of(&self.canonical_json())
    }

    pub fn file(&self, raw: RawInputs) -> PlanFile {
        let programs = self
            .programs
            .iter()
            .map(|(name, program)| (name.clone(), program.into()))
            .collect();

        PlanFile {
            document: PlanDocument {
                release: self.release.clone(),
                inputs: raw,
            },
            genesis_hash: self.genesis_hash.to_string(),
            hash: self.hash().to_string(),
            programs,
            live_configs: self
                .live_configs
                .entries()
                .into_iter()
                .map(|(program, values)| (program.into(), values))
                .collect(),
        }
    }

    pub fn summary(&self) -> String {
        let Self {
            release,
            programs,
            inputs,
            ..
        } = self;
        let table = programs.iter().fold(
            "| Program | Address | Status | Actions |\n|---|---|---|---|\n".to_owned(),
            |table, (name, program)| {
                let actions = match program.actions.is_empty() {
                    true => "none".to_owned(),
                    false => join_displayed(program.actions.iter().map(action_label)),
                };
                let status: PlannedStatus = (&program.state.status).into();

                table
                    + &format!(
                        "| {name} | `{}` | {status} | {actions} |\n",
                        program.state.address
                    )
            },
        );

        format!(
            "## Deploy plan v{} on {}\n\n{table}\n{}\nPlan hash: {}\n",
            release.version,
            release.cluster,
            init_values(inputs),
            self.hash(),
        )
    }

    fn canonical_json(&self) -> String {
        let Self {
            release,
            genesis_hash,
            deployer,
            programs,
            inputs,
            live_configs,
            ..
        } = self;
        let canonical = CanonicalPlan {
            version: &release.version,
            cluster: release.cluster,
            genesis_hash: genesis_hash.to_string(),
            deployer: deployer.to_string(),
            programs: programs
                .iter()
                .map(|(name, program)| {
                    (
                        name.as_str(),
                        canonical_program(&release.programs[name], program),
                    )
                })
                .collect(),
            inputs: inputs.into(),
            live_configs: live_configs.entries().into_iter().collect(),
            live_layerzero_alt: live_configs.layerzero_alt.as_ref().map(Alt::describe),
            live_layerzero_paths: live_configs
                .layerzero_paths
                .iter()
                .map(Path::describe)
                .collect(),
        };

        serde_json::to_string(&canonical).expect("canonical plan must serialize")
    }
}

impl From<&PlannedProgram> for PlannedProgramFile {
    fn from(program: &PlannedProgram) -> Self {
        let ProgramState {
            status,
            authority,
            data_hash,
            ..
        } = &program.state;

        Self {
            status: status.into(),
            authority: authority.map(|authority| authority.to_string()),
            data_hash: data_hash.map(hex::encode),
            actions: program.actions.clone(),
        }
    }
}

impl Release {
    /// Every program after all of its dependencies, ties in name order. `program-ids.json` comes
    /// from Cargo's dependency graph plus the aggregator's members, so it has no cycles.
    pub fn dependency_order(&self) -> Vec<&str> {
        std::iter::successors(Some(Vec::<&str>::new()), |placed| {
            let next: Vec<&str> = self
                .programs
                .iter()
                .filter(|(name, program)| {
                    !placed.contains(&name.as_str())
                        && program
                            .dependencies
                            .iter()
                            .all(|dependency| placed.contains(&dependency.as_str()))
                })
                .map(|(name, _)| name.as_str())
                .collect();

            (!next.is_empty()).then(|| [placed.clone(), next].concat())
        })
        .last()
        .filter(|placed| placed.len() == self.programs.len())
        .expect("release dependencies must be acyclic and name released programs")
    }
}

impl From<&Status> for PlannedStatus {
    fn from(status: &Status) -> Self {
        match status {
            Status::New => Self::New,
            Status::Partial => Self::Partial,
            Status::Live => Self::Live,
            Status::Foreign { .. } => Self::Foreign,
        }
    }
}

impl fmt::Display for PlannedStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::New => "new",
            Self::Partial => "partial",
            Self::Live => "live",
            Self::Foreign => "foreign",
        })
    }
}

impl Cluster {
    pub fn genesis_hash(self) -> Hash {
        let cluster: ClusterType = self.into();

        cluster
            .get_genesis_hash()
            .expect("devnet and mainnet have a known genesis hash")
    }

    /// Devnet and mainnet share program IDs, so only the genesis hash tells them apart.
    pub fn require_genesis(self, actual: Hash) -> Result<(), Error> {
        let expected = self.genesis_hash();

        match actual == expected {
            true => Ok(()),
            false => Err(Error::ClusterMismatch {
                cluster: self,
                expected,
                actual,
            }),
        }
    }
}

impl From<Cluster> for ClusterType {
    fn from(cluster: Cluster) -> Self {
        match cluster {
            Cluster::Devnet => Self::Devnet,
            Cluster::Mainnet => Self::MainnetBeta,
        }
    }
}

impl fmt::Display for Cluster {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Devnet => "devnet",
            Self::Mainnet => "mainnet",
        })
    }
}

impl FromStr for Cluster {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "devnet" => Ok(Self::Devnet),
            "mainnet" => Ok(Self::Mainnet),
            _ => Err(Error::UnknownCluster {
                value: value.into(),
            }),
        }
    }
}

impl PlanHash {
    /// The hash of a plan's canonical JSON document.
    pub fn of(canonical_json: &str) -> Self {
        Self(solana_sha256_hasher::hash(canonical_json.as_bytes()).to_bytes())
    }
}

impl fmt::Display for PlanHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl FromStr for PlanHash {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        hex::decode(value)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(Self)
            .ok_or_else(|| Error::InvalidPlanHash {
                value: value.into(),
            })
    }
}

fn expected_configs(release: &Release, inputs: &Inputs) -> Result<Configs, Error> {
    let member_address = |name: &str| {
        release
            .programs
            .get(name)
            .map(|program| program.address)
            .ok_or_else(|| Error::MissingState {
                program: name.into(),
            })
    };
    let to_bytes32 =
        |addresses: &[EvmAddress]| -> Vec<Bytes32> { addresses.iter().map(Into::into).collect() };

    let layerzero = member_address(LAYERZERO_PROVER)?;

    Ok(Configs {
        hyper_senders: Some(to_bytes32(&inputs.hyper_senders)),
        polymer_emitters: Some(to_bytes32(&inputs.polymer_emitters)),
        aggregator_provers: Some(
            AGGREGATOR_MEMBERS
                .into_iter()
                .map(member_address)
                .collect::<Result<_, _>>()?,
        ),
        layerzero_peers: Some(inputs.layerzero_peers.iter().map(Into::into).collect()),
        layerzero_alt: None,
        layerzero_paths: inputs
            .layerzero_peers
            .iter()
            .map(|peer| Path::expected(&layerzero, &peer.into(), &peer.path))
            .collect(),
    })
}

/// The lookup table and its on-chain check derive from the compiled-in ID, so the deployer
/// must be built from the tree that compiled the release's LayerZero prover.
fn require_compiled_layerzero(release: &Release) -> Result<(), Error> {
    let release = release.programs[LAYERZERO_PROVER].address;

    match release == layerzero_prover::ID {
        true => Ok(()),
        false => Err(Error::BuiltForAnotherRelease {
            release,
            compiled: layerzero_prover::ID,
        }),
    }
}

fn reject_foreign(name: &str, program: &ReleaseProgram, state: &ProgramState) -> Result<(), Error> {
    if let Status::Foreign { reason } = &state.status {
        return Err(Error::Foreign {
            program: name.into(),
            reason: reason.clone(),
        });
    }
    match program.address == state.address {
        true => Ok(()),
        false => Err(Error::AddressMismatch {
            program: name.into(),
            release: program.address,
            state: state.address,
        }),
    }
}

/// A deploy run never finalizes: programs stay upgradeable until a finalize run, after testing.
fn actions(name: &str, status: &Status) -> Vec<Action> {
    let deploy = matches!(status, Status::New).then_some(Action::Deploy);
    let init = PROGRAMS_WITH_INIT.contains(&name).then_some(Action::Init);

    match status {
        Status::Live | Status::Foreign { .. } => vec![],
        Status::New | Status::Partial => [deploy, init, Some(Action::Verify)]
            .into_iter()
            .flatten()
            .collect(),
    }
}

fn action_label(action: &Action) -> &'static str {
    match action {
        Action::Deploy => "deploy",
        Action::Init => "init",
        Action::Verify => "verify",
        Action::Finalize => "finalize",
        Action::Close => "close",
    }
}

fn join_displayed<T: fmt::Display>(items: impl IntoIterator<Item = T>) -> String {
    items
        .into_iter()
        .map(|item| item.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn init_values(inputs: &Inputs) -> String {
    let Inputs {
        hyper_senders,
        polymer_emitters,
        layerzero_peers,
        hyper_reserve_lamports,
        layerzero_reserve_lamports,
        compute_unit_price,
    } = inputs;
    let header = format!(
        "### Init values\n\n\
         - compute unit price (micro-lamports): {compute_unit_price}\n\
         - {HYPER_PROVER} senders: {}\n\
         - {HYPER_PROVER} reserve lamports: {hyper_reserve_lamports}\n\
         - {POLYMER_PROVER} emitters: {}\n\
         - {AGGREGATOR_PROVER} members: {}\n\
         - {LAYERZERO_PROVER} reserve lamports: {layerzero_reserve_lamports}\n",
        join_displayed(hyper_senders),
        join_displayed(polymer_emitters),
        AGGREGATOR_MEMBERS.join(", "),
    );

    layerzero_peers
        .iter()
        .fold(header, |values, peer| values + &peer_summary(peer))
}

fn peer_summary(peer: &LayerZeroPeer) -> String {
    let LayerZeroPeer {
        eid,
        address,
        chain_id,
        path,
    } = peer;
    let uln = |label: &str, uln: &UlnConfig| {
        format!(
            "  - {label}: confirmations {}, required DVNs [{}], optional DVNs [{}] (threshold {}, count {})\n",
            uln.confirmations,
            join_displayed(&uln.required_dvns),
            join_displayed(&uln.optional_dvns),
            uln.optional_dvn_threshold,
            uln.optional_dvn_count,
        )
    };

    format!(
        "- {LAYERZERO_PROVER} peer eid {eid}: {address}, chain id {chain_id}\n{}{}  - executor: {} (max message size {})\n",
        uln("send", &path.send_uln),
        uln("receive", &path.receive_uln),
        path.executor.executor,
        path.executor.max_message_size,
    )
}

#[derive(Serialize)]
struct CanonicalPlan<'a> {
    version: &'a str,
    cluster: Cluster,
    genesis_hash: String,
    deployer: String,
    programs: BTreeMap<&'a str, CanonicalProgram>,
    inputs: CanonicalInputs,
    live_configs: BTreeMap<&'static str, Option<Vec<String>>>,
    live_layerzero_alt: Option<String>,
    live_layerzero_paths: Vec<String>,
}

#[derive(Serialize)]
struct CanonicalProgram {
    address: String,
    so_sha256: String,
    dependencies: Vec<String>,
    status: PlannedStatus,
    authority: Option<String>,
    data_hash: Option<String>,
    actions: Vec<Action>,
}

#[derive(Serialize)]
struct CanonicalInputs {
    hyper_senders: Vec<String>,
    polymer_emitters: Vec<String>,
    layerzero_peers: Vec<CanonicalPeer>,
    hyper_reserve_lamports: u64,
    layerzero_reserve_lamports: u64,
    compute_unit_price: u64,
}

#[derive(Serialize)]
struct CanonicalPeer {
    eid: u32,
    address: String,
    chain_id: u64,
    send_uln: CanonicalUln,
    receive_uln: CanonicalUln,
    executor: CanonicalExecutor,
}

#[derive(Serialize)]
struct CanonicalUln {
    confirmations: u64,
    required_dvn_count: u8,
    optional_dvn_count: u8,
    optional_dvn_threshold: u8,
    required_dvns: Vec<String>,
    optional_dvns: Vec<String>,
}

#[derive(Serialize)]
struct CanonicalExecutor {
    max_message_size: u32,
    executor: String,
}

fn canonical_program(release: &ReleaseProgram, program: &PlannedProgram) -> CanonicalProgram {
    let ProgramState {
        address,
        status,
        authority,
        data_hash,
    } = &program.state;

    CanonicalProgram {
        address: address.to_string(),
        so_sha256: hex::encode(release.so_sha256),
        dependencies: release.dependencies.clone(),
        status: status.into(),
        authority: authority.map(|authority| authority.to_string()),
        data_hash: data_hash.map(hex::encode),
        actions: program.actions.clone(),
    }
}

impl From<&Inputs> for CanonicalInputs {
    fn from(inputs: &Inputs) -> Self {
        Self {
            hyper_senders: inputs
                .hyper_senders
                .iter()
                .map(ToString::to_string)
                .collect(),
            polymer_emitters: inputs
                .polymer_emitters
                .iter()
                .map(ToString::to_string)
                .collect(),
            layerzero_peers: inputs.layerzero_peers.iter().map(Into::into).collect(),
            hyper_reserve_lamports: inputs.hyper_reserve_lamports,
            layerzero_reserve_lamports: inputs.layerzero_reserve_lamports,
            compute_unit_price: inputs.compute_unit_price,
        }
    }
}

impl From<&LayerZeroPeer> for CanonicalPeer {
    fn from(peer: &LayerZeroPeer) -> Self {
        let PathConfig {
            send_uln,
            receive_uln,
            executor,
        } = &peer.path;

        Self {
            eid: peer.eid,
            address: peer.address.to_string(),
            chain_id: peer.chain_id,
            send_uln: send_uln.into(),
            receive_uln: receive_uln.into(),
            executor: executor.into(),
        }
    }
}

impl From<&UlnConfig> for CanonicalUln {
    fn from(uln: &UlnConfig) -> Self {
        Self {
            confirmations: uln.confirmations,
            required_dvn_count: uln.required_dvn_count,
            optional_dvn_count: uln.optional_dvn_count,
            optional_dvn_threshold: uln.optional_dvn_threshold,
            required_dvns: uln.required_dvns.iter().map(ToString::to_string).collect(),
            optional_dvns: uln.optional_dvns.iter().map(ToString::to_string).collect(),
        }
    }
}

impl From<&ExecutorConfig> for CanonicalExecutor {
    fn from(executor: &ExecutorConfig) -> Self {
        Self {
            max_message_size: executor.max_message_size,
            executor: executor.executor.to_string(),
        }
    }
}

mod pubkey_string {
    use serde::{de, Deserialize, Deserializer, Serializer};
    use solana_sdk::pubkey::Pubkey;

    pub fn serialize<S: Serializer>(pubkey: &Pubkey, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(pubkey)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Pubkey, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

mod hex_array {
    use serde::{de, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        hex::decode(String::deserialize(deserializer)?)
            .map_err(de::Error::custom)?
            .try_into()
            .map_err(|_| de::Error::custom("expected 32 bytes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_ADDRESS: &str = "0xAbCdEf0123456789aBcDeF0123456789abcdef01";
    const OTHER_ADDRESS: &str = "0x1111111111111111111111111111111111111111";
    const DVN: &str = "11111111111111111111111111111112";
    const OTHER_DVN: &str = "11111111111111111111111111111114";
    const EXECUTOR: &str = "11111111111111111111111111111113";
    const PROGRAMS: [&str; 8] = [
        "aggregator_prover",
        "flash_fulfiller",
        "hyper_prover",
        "layerzero_prover",
        "local_prover",
        "polymer_prover",
        "portal",
        "proof_helper",
    ];

    type ConfigChange = fn(&mut Configs);

    #[derive(Clone)]
    struct Fixture {
        release: Release,
        genesis_hash: Hash,
        states: BTreeMap<String, ProgramState>,
        configs: Configs,
        raw: RawInputs,
    }

    impl Fixture {
        fn plan(&self) -> Result<Plan, Error> {
            let Self {
                release,
                genesis_hash,
                states,
                configs,
                raw,
            } = self.clone();

            Plan::build(
                release,
                genesis_hash,
                deployer(),
                states,
                configs,
                Inputs::parse(raw).unwrap(),
            )
        }

        fn with_expected_configs(self) -> Self {
            let configs = Configs {
                layerzero_alt: Some(Alt {
                    address: address(9),
                    fault: None,
                }),
                ..self.plan().unwrap().expected_configs
            };

            Self { configs, ..self }
        }

        fn hash(&self) -> PlanHash {
            self.plan().unwrap().hash()
        }

        fn state(&mut self, program: &str) -> &mut ProgramState {
            self.states.get_mut(program).unwrap()
        }
    }

    fn address(index: usize) -> Pubkey {
        Pubkey::new_from_array([index as u8 + 1; 32])
    }

    fn deployer() -> Pubkey {
        Pubkey::new_from_array([99; 32])
    }

    /// The release's real dependency graph.
    fn fixture_dependencies(program: &str) -> Vec<String> {
        let dependencies: &[&str] = match program {
            AGGREGATOR_PROVER => &[HYPER_PROVER, LAYERZERO_PROVER, POLYMER_PROVER, "portal"],
            LOCAL_PROVER => &["flash_fulfiller", "portal"],
            "portal" | "proof_helper" => &[],
            _ => &["portal"],
        };

        dependencies.iter().map(|name| (*name).into()).collect()
    }

    fn fixture_address(index: usize, program: &str) -> Pubkey {
        match program {
            LAYERZERO_PROVER => layerzero_prover::ID,
            _ => address(index),
        }
    }

    fn uln_json() -> String {
        format!(
            r#"{{"confirmations":15,"required_dvn_count":1,"optional_dvn_count":255,"optional_dvn_threshold":0,"required_dvns":["{DVN}"],"optional_dvns":[]}}"#
        )
    }

    fn peer_json(eid: u32, address: &str) -> String {
        let uln = uln_json();

        format!(
            r#"{{"eid":{eid},"address":"{address}","chain_id":{eid},"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}"#
        )
    }

    fn raw_inputs() -> RawInputs {
        RawInputs {
            hyper_senders: format!("{VALID_ADDRESS}, {OTHER_ADDRESS}"),
            polymer_emitters: OTHER_ADDRESS.into(),
            layerzero: format!(
                r#"{{"peers":[{},{}]}}"#,
                peer_json(30184, VALID_ADDRESS),
                peer_json(30101, OTHER_ADDRESS)
            ),
            hyper_reserve_lamports: "1000000".into(),
            layerzero_reserve_lamports: "2000000".into(),
            compute_unit_price: "0".into(),
        }
    }

    fn so_bytes(program: &str) -> Vec<u8> {
        format!("{program} bytecode").into_bytes()
    }

    fn fixture() -> Fixture {
        let release = Release {
            version: "1.2.3".into(),
            cluster: Cluster::Devnet,
            programs: PROGRAMS
                .iter()
                .enumerate()
                .map(|(index, program)| {
                    (
                        (*program).into(),
                        ReleaseProgram {
                            address: fixture_address(index, program),
                            so_sha256: solana_sha256_hasher::hash(&so_bytes(program)).to_bytes(),
                            dependencies: fixture_dependencies(program),
                        },
                    )
                })
                .collect(),
        };
        let states = PROGRAMS
            .iter()
            .enumerate()
            .map(|(index, program)| {
                let (status, authority, data_hash) = match *program {
                    "portal" => (Status::Live, None, Some([7; 32])),
                    "layerzero_prover" | "hyper_prover" => {
                        (Status::Partial, Some(deployer()), Some([8; 32]))
                    }
                    _ => (Status::New, None, None),
                };

                (
                    (*program).into(),
                    ProgramState {
                        address: fixture_address(index, program),
                        status,
                        authority,
                        data_hash,
                    },
                )
            })
            .collect();

        Fixture {
            release,
            genesis_hash: Cluster::Devnet.genesis_hash(),
            states,
            configs: Configs::default(),
            raw: raw_inputs(),
        }
    }

    fn actions_of(plan: &Plan, program: &str) -> Vec<Action> {
        plan.programs[program].actions.clone()
    }

    #[test]
    fn plan_hash_is_stable() {
        let plan = fixture().plan().unwrap();

        goldie::assert!(format!("{}\n{}", plan.hash(), plan.canonical_json()));
    }

    #[test]
    fn plan_rejects_a_chain_of_another_cluster() {
        let mainnet = Fixture {
            genesis_hash: Cluster::Mainnet.genesis_hash(),
            ..fixture()
        };

        assert!(matches!(
            mainnet.plan(),
            Err(Error::ClusterMismatch {
                cluster: Cluster::Devnet,
                ..
            })
        ));
    }

    #[test]
    fn plan_rejects_a_release_the_deployer_was_not_built_for() {
        let mut other = fixture();
        other
            .release
            .programs
            .get_mut(LAYERZERO_PROVER)
            .unwrap()
            .address = address(42);

        assert!(matches!(
            other.plan(),
            Err(Error::BuiltForAnotherRelease { release, compiled })
                if release == address(42) && compiled == layerzero_prover::ID
        ));
    }

    #[test]
    fn aggregator_members_mirror_the_keypair_script_in_order() {
        let script = include_str!("../../../scripts/program-keypairs.mjs");
        let members = AGGREGATOR_MEMBERS.map(|member| format!(r#""{member}""#));

        assert!(script.contains(&format!(
            "const AGGREGATOR_MEMBERS = [{}];",
            members.join(", ")
        )));
    }

    #[test]
    fn plan_hash_covers_the_compute_unit_price() {
        let priced = Fixture {
            raw: RawInputs {
                compute_unit_price: "1".into(),
                ..raw_inputs()
            },
            ..fixture()
        };

        assert_ne!(priced.hash(), fixture().hash());
    }

    #[test]
    fn plan_hash_covers_the_deployer() {
        let plan = fixture().plan().unwrap();

        assert!(plan
            .canonical_json()
            .contains(&format!(r#""deployer":"{}""#, deployer())));
    }

    #[test]
    fn plan_hash_covers_the_genesis_hash() {
        let plan = fixture().plan().unwrap();

        assert!(plan
            .canonical_json()
            .contains(r#""genesis_hash":"EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG""#));
    }

    #[test]
    fn cluster_genesis_hashes_are_the_published_ones() {
        assert_eq!(
            [Cluster::Devnet, Cluster::Mainnet].map(|cluster| cluster.genesis_hash().to_string()),
            [
                "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG",
                "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d",
            ]
        );
    }

    #[test]
    fn plan_hash_ignores_address_case_and_whitespace() {
        let baseline = fixture().hash();
        let reformatted = Fixture {
            raw: RawInputs {
                hyper_senders: format!("  {},\n{}  ", VALID_ADDRESS.to_lowercase(), OTHER_ADDRESS),
                polymer_emitters: format!(" {} ", OTHER_ADDRESS.to_uppercase().replace("0X", "0x")),
                layerzero: raw_inputs().layerzero.replace(',', " ,\n ").replace(
                    &VALID_ADDRESS.to_lowercase(),
                    &VALID_ADDRESS.to_uppercase().replace("0X", "0x"),
                ),
                hyper_reserve_lamports: " 1000000\n".into(),
                layerzero_reserve_lamports: "2000000 ".into(),
                ..raw_inputs()
            },
            ..fixture()
        };

        assert_eq!(reformatted.hash(), baseline);
    }

    #[test]
    fn plan_hash_changes_with_each_input() {
        type Mutation = fn(&mut Fixture);
        let mutations: Vec<(&str, Mutation)> = vec![
            ("one sender", |fixture| {
                fixture.raw.hyper_senders =
                    format!("{VALID_ADDRESS}, 0x2222222222222222222222222222222222222222")
            }),
            ("sender order", |fixture| {
                fixture.raw.hyper_senders = format!("{OTHER_ADDRESS}, {VALID_ADDRESS}")
            }),
            ("emitter", |fixture| {
                fixture.raw.polymer_emitters = VALID_ADDRESS.into()
            }),
            ("peer eid", |fixture| {
                fixture.raw.layerzero = fixture.raw.layerzero.replacen("30184", "30185", 1)
            }),
            ("peer chain id", |fixture| {
                fixture.raw.layerzero =
                    fixture
                        .raw
                        .layerzero
                        .replacen("\"chain_id\":30184", "\"chain_id\":1", 1)
            }),
            ("peer order", |fixture| {
                fixture.raw.layerzero = format!(
                    r#"{{"peers":[{},{}]}}"#,
                    peer_json(30101, OTHER_ADDRESS),
                    peer_json(30184, VALID_ADDRESS)
                )
            }),
            ("dvn", |fixture| {
                fixture.raw.layerzero = fixture.raw.layerzero.replacen(DVN, OTHER_DVN, 1)
            }),
            ("confirmations", |fixture| {
                fixture.raw.layerzero = fixture.raw.layerzero.replacen(
                    "\"confirmations\":15",
                    "\"confirmations\":16",
                    1,
                )
            }),
            ("executor", |fixture| {
                fixture.raw.layerzero = fixture.raw.layerzero.replacen(EXECUTOR, OTHER_DVN, 1)
            }),
            ("hyper reserve", |fixture| {
                fixture.raw.hyper_reserve_lamports = "1000001".into()
            }),
            ("layerzero reserve", |fixture| {
                fixture.raw.layerzero_reserve_lamports = "2000001".into()
            }),
            ("cluster", |fixture| {
                fixture.release.cluster = Cluster::Mainnet;
                fixture.genesis_hash = Cluster::Mainnet.genesis_hash();
            }),
            ("version", |fixture| {
                fixture.release.version = "1.2.4".into()
            }),
            ("so byte", |fixture| {
                let mut bytes = so_bytes("portal");
                bytes[0] ^= 1;
                fixture
                    .release
                    .programs
                    .get_mut("portal")
                    .unwrap()
                    .so_sha256 = solana_sha256_hasher::hash(&bytes).to_bytes();
            }),
            ("status", |fixture| {
                fixture.state("proof_helper").status = Status::Partial
            }),
            ("authority", |fixture| {
                fixture.state("hyper_prover").authority = Some(address(50))
            }),
            ("data hash", |fixture| {
                fixture.state("portal").data_hash = Some([9; 32])
            }),
        ];
        let baseline = fixture().hash();

        mutations.iter().for_each(|(name, mutate)| {
            let mut mutated = fixture();
            mutate(&mut mutated);

            assert_ne!(mutated.hash(), baseline, "{name}");
        });
    }

    #[test]
    fn plan_hash_changes_with_live_configs() {
        let baseline = fixture().hash();
        let matching = fixture().with_expected_configs();
        let hyper_only = Fixture {
            configs: Configs {
                hyper_senders: matching.configs.hyper_senders.clone(),
                ..Configs::default()
            },
            ..matching.clone()
        };

        assert_ne!(matching.hash(), baseline);
        assert_ne!(hyper_only.hash(), baseline);
        assert_ne!(hyper_only.hash(), matching.hash());
    }

    #[test]
    fn plan_hash_changes_with_live_layerzero_alt() {
        let matching = fixture().with_expected_configs();
        let with_alt = Fixture {
            configs: Configs {
                layerzero_alt: Some(Alt {
                    address: address(7),
                    fault: None,
                }),
                ..matching.configs.clone()
            },
            ..matching.clone()
        };

        assert_ne!(with_alt.hash(), matching.hash());
    }

    #[test]
    fn live_program_without_config_fails_plan() {
        [
            HYPER_PROVER,
            POLYMER_PROVER,
            AGGREGATOR_PROVER,
            LAYERZERO_PROVER,
        ]
        .into_iter()
        .for_each(|program| {
            let mut live_without_config = fixture();
            live_without_config.state(program).status = Status::Live;

            assert!(matches!(
                live_without_config.plan(),
                Err(Error::LiveWithoutConfig { program: failed }) if failed == program
            ));
        });
    }

    #[test]
    fn live_layerzero_missing_any_part_of_its_setup_fails_plan() {
        let missing: [(&str, ConfigChange); 3] = [
            ("alt", |configs| configs.layerzero_alt = None),
            ("nonce", |configs| configs.layerzero_paths[0].nonce = false),
            ("receive config", |configs| {
                configs.layerzero_paths[0].receive = None
            }),
        ];

        missing.into_iter().for_each(|(name, change)| {
            let mut live = fixture().with_expected_configs();
            live.state(LAYERZERO_PROVER).status = Status::Live;
            change(&mut live.configs);

            assert!(
                matches!(
                    live.plan(),
                    Err(Error::LiveWithoutConfig { program }) if program == LAYERZERO_PROVER
                ),
                "{name}"
            );
        });
    }

    #[test]
    fn live_program_with_matching_config_plans() {
        let mut live = fixture().with_expected_configs();
        live.state(HYPER_PROVER).status = Status::Live;
        live.state(LAYERZERO_PROVER).status = Status::Live;

        assert!(live.plan().is_ok());
    }

    #[test]
    fn live_configs_equal_to_inputs_plan() {
        assert!(fixture().with_expected_configs().plan().is_ok());
    }

    #[test]
    fn live_config_differing_from_inputs_fails_plan() {
        let matching = fixture().with_expected_configs().configs;
        let other: Vec<Bytes32> = vec![[9; 32].into()];
        let differing: Vec<(&str, Configs)> = vec![
            (
                HYPER_PROVER,
                Configs {
                    hyper_senders: Some(other.clone()),
                    ..matching.clone()
                },
            ),
            (
                POLYMER_PROVER,
                Configs {
                    polymer_emitters: Some(other),
                    ..matching.clone()
                },
            ),
            (
                AGGREGATOR_PROVER,
                Configs {
                    aggregator_provers: Some(vec![address(0)]),
                    ..matching.clone()
                },
            ),
            (
                LAYERZERO_PROVER,
                Configs {
                    layerzero_paths: {
                        let mut paths = matching.layerzero_paths.clone();
                        paths[0].send.as_mut().unwrap().uln.confirmations += 1;
                        paths
                    },
                    ..matching.clone()
                },
            ),
            (
                LAYERZERO_PROVER,
                Configs {
                    layerzero_peers: matching.layerzero_peers.clone().map(|mut peers| {
                        peers[0].chain_id += 1;
                        peers
                    }),
                    ..matching
                },
            ),
        ];

        differing.into_iter().for_each(|(program, configs)| {
            [Status::New, Status::Partial, Status::Live]
                .into_iter()
                .for_each(|status| {
                    let mut mismatched = Fixture {
                        configs: configs.clone(),
                        ..fixture()
                    };
                    mismatched.state(program).status = status;

                    assert!(matches!(
                        mismatched.plan(),
                        Err(Error::ConfigMismatch(mismatch)) if mismatch.program == program
                    ));
                });
        });
    }

    #[test]
    fn foreign_program_fails_plan() {
        let mut foreign = fixture();
        foreign.state("portal").status = Status::Foreign {
            reason: "hash differs".into(),
        };

        assert!(matches!(
            foreign.plan(),
            Err(Error::Foreign { program, reason }) if program == "portal" && reason == "hash differs"
        ));
    }

    #[test]
    fn missing_state_and_address_mismatch_fail_plan() {
        let mut missing = fixture();
        missing.states.remove("portal");
        let mut mismatched = fixture();
        mismatched.state("portal").address = address(50);

        assert!(matches!(missing.plan(), Err(Error::MissingState { .. })));
        assert!(matches!(
            mismatched.plan(),
            Err(Error::AddressMismatch { .. })
        ));
    }

    #[test]
    fn actions_follow_status_and_init() {
        let plan = fixture().plan().unwrap();

        assert_eq!(actions_of(&plan, "portal"), vec![],);
        assert_eq!(
            actions_of(&plan, "proof_helper"),
            vec![Action::Deploy, Action::Verify]
        );
        assert_eq!(
            actions_of(&plan, "polymer_prover"),
            vec![Action::Deploy, Action::Init, Action::Verify]
        );
        assert_eq!(
            actions_of(&plan, "hyper_prover"),
            vec![Action::Init, Action::Verify]
        );
    }

    #[test]
    fn deploy_never_finalizes() {
        let plan = fixture().plan().unwrap();

        assert!(plan
            .programs
            .values()
            .all(|program| !program.actions.contains(&Action::Finalize)));
    }

    #[test]
    fn plan_document_round_trips_to_the_same_hash() {
        let Fixture { release, raw, .. } = fixture();
        let document = serde_json::to_string_pretty(&PlanDocument {
            release,
            inputs: raw,
        })
        .unwrap();
        let PlanDocument { release, inputs } = serde_json::from_str(&document).unwrap();
        let rebuilt = Plan::build(
            release,
            Cluster::Devnet.genesis_hash(),
            deployer(),
            fixture().states,
            Configs::default(),
            Inputs::parse(inputs).unwrap(),
        )
        .unwrap();

        assert_eq!(rebuilt.hash(), fixture().hash());
    }

    #[test]
    fn plan_hash_and_cluster_parse_and_display() {
        let hash = fixture().hash();

        assert_eq!(hash.to_string().parse::<PlanHash>().unwrap(), hash);
        assert!("abc".parse::<PlanHash>().is_err());
        assert!("zz".repeat(32).parse::<PlanHash>().is_err());
        assert_eq!("mainnet".parse::<Cluster>().unwrap().to_string(), "mainnet");
        assert_eq!("devnet".parse::<Cluster>().unwrap().to_string(), "devnet");
        assert!("testnet".parse::<Cluster>().is_err());
    }

    #[test]
    fn summary_lists_actions_and_hash() {
        goldie::assert!(fixture().plan().unwrap().summary());
    }

    fn sorted_dvns_json(count: u8) -> String {
        (1..=count)
            .map(|index| {
                let mut bytes = [0u8; 32];
                bytes[31] = index;

                format!(r#""{}""#, Pubkey::new_from_array(bytes))
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    fn fixture_with_dvns(count: u8) -> Fixture {
        let uln = format!(
            r#"{{"confirmations":15,"required_dvn_count":{count},"optional_dvn_count":255,"optional_dvn_threshold":0,"required_dvns":[{}],"optional_dvns":[]}}"#,
            sorted_dvns_json(count)
        );
        let peer = format!(
            r#"{{"eid":30184,"address":"{VALID_ADDRESS}","chain_id":30184,"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}"#
        );

        Fixture {
            raw: RawInputs {
                layerzero: format!(r#"{{"peers":[{peer}]}}"#),
                ..raw_inputs()
            },
            ..fixture()
        }
    }

    #[test]
    fn the_largest_path_config_that_fits_a_packet_is_planned() {
        assert!(fixture_with_dvns(7).plan().is_ok());
    }

    #[test]
    fn the_maximum_number_of_peers_fits_one_init_transaction() {
        let peers = (1..=layerzero_prover::state::MAX_PEERS as u32)
            .map(|eid| peer_json(eid, &format!("0x{eid:040x}")))
            .collect::<Vec<_>>()
            .join(",");
        let fixture = Fixture {
            raw: RawInputs {
                layerzero: format!(r#"{{"peers":[{peers}]}}"#),
                ..raw_inputs()
            },
            ..fixture()
        };

        assert!(fixture.plan().is_ok());
    }

    #[test]
    fn a_path_config_over_the_packet_limit_is_rejected_at_plan() {
        let error = fixture_with_dvns(8).plan().unwrap_err();

        assert!(
            matches!(
                &error,
                Error::LayerZero(layerzero::Error::TransactionTooLarge { transaction, size, limit })
                    if transaction == "set_path_config for eid 30184" && *size == 1284 && *limit == 1232
            ),
            "{error:?}"
        );
    }
}
