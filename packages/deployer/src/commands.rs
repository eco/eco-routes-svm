use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Keypair};
use solana_sdk::signer::Signer;

use crate::apply::{self, Step};
use crate::chain::{self, Chain, Priced};
use crate::classify::{self, classify, ProgramState, Status};
use crate::cli::{ActionsArgs, ApplyArgs, Command, PlanArgs};
use crate::inputs::{self, Inputs, RawInputs};
use crate::plan::{
    self, Cluster, Plan, PlanDocument, PlanFile, PlanHash, PlannedProgramFile, PlannedStatus,
    Release, ReleaseProgram,
};
use crate::rpc::RpcChain;
use crate::{config, readback};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Apply(#[from] apply::Error),
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error(transparent)]
    Classify(#[from] classify::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error(transparent)]
    Inputs(#[from] inputs::Error),
    #[error(transparent)]
    Plan(#[from] plan::Error),
    #[error(transparent)]
    Readback(#[from] readback::Error),
    #[error("plan {expected} no longer matches the chain (now {actual}); plan again and review the new hash")]
    PlanChanged {
        expected: PlanHash,
        actual: PlanHash,
    },
    #[error("{program}: release binary {path} hashes to {actual}, the plan recorded {expected}")]
    AssetChanged {
        program: String,
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("{program}: {value:?} in program-ids.json is not an address")]
    InvalidAddress { program: String, value: String },
    #[error("cannot read deployer keypair {path}")]
    Keypair {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error>,
    },
    #[error("cannot read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("cannot write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("{path} is not valid: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("cannot write to stdout")]
    Output {
        #[source]
        source: io::Error,
    },
}

/// The `program-ids.json` entry fields the plan needs; `seed` and `salt` only matter to
/// `scripts/program-keypairs.mjs`.
#[derive(Deserialize)]
struct ProgramId {
    address: String,
}

type Binaries = BTreeMap<String, Vec<u8>>;

pub fn run(command: &Command, out: &mut impl Write) -> Result<(), Error> {
    match command {
        Command::Plan(args) => {
            let deployer = read_keypair(&args.chain.deployer_keypair)?;
            let chain = RpcChain::new(args.chain.rpc_url.clone());

            plan(&chain, args, &deployer.pubkey(), out)
        }
        Command::Apply(args) => {
            let deployer = read_keypair(&args.chain.deployer_keypair)?;
            let rpc = RpcChain::new(args.chain.rpc_url.clone());
            let mut chain = Priced::new(rpc, args.compute_unit_price);

            apply(&mut chain, args, &deployer, out)
        }
        Command::Actions(args) => actions(args, out),
    }
}

pub fn plan(
    chain: &impl Chain,
    args: &PlanArgs,
    deployer: &Pubkey,
    out: &mut impl Write,
) -> Result<(), Error> {
    let PlanArgs {
        version,
        cluster,
        program_ids,
        assets,
        out: plan_path,
        summary,
        inputs,
        ..
    } = args;
    let (release, binaries) = release(version, *cluster, program_ids, assets)?;
    let raw = RawInputs {
        hyper_senders: inputs.hyper_senders.clone(),
        polymer_emitters: inputs.polymer_emitters.clone(),
        layerzero: inputs.layerzero.clone(),
        hyper_reserve_lamports: inputs.hyper_reserve_lamports.clone(),
        layerzero_reserve_lamports: inputs.layerzero_reserve_lamports.clone(),
        finalize_layerzero: inputs.finalize_layerzero,
    };
    let document = PlanDocument {
        release,
        inputs: raw.clone(),
    };
    let plan = build(chain, &document, &binaries, deployer, &BTreeMap::new())?;

    write_json(plan_path, &plan.file(raw))?;
    summary
        .iter()
        .try_for_each(|path| append(path, &plan.summary()))?;

    print_hash(out, &plan.hash())
}

/// Rebuilds the plan from the chain as it is now and refuses unless it hashes to the one in
/// `plan.json`; the only change tolerated is the `deploy` step turning a planned-`New` program
/// into a `Partial` one.
pub fn apply(
    chain: &mut impl Chain,
    args: &ApplyArgs,
    deployer: &Keypair,
    out: &mut impl Write,
) -> Result<(), Error> {
    let (live, expected) = rebuild(chain, args, &deployer.pubkey())?;
    let mut printed = Ok(());
    let landed = apply::apply_observed(chain, &live, deployer, &mut |step| {
        printed = mem::replace(&mut printed, Ok(())).and_then(|()| print_step(out, step));
    });
    printed?;
    landed?;
    let read_back = readback::readback(chain, &live)?;
    write!(out, "read back:\n{read_back}").map_err(|source| Error::Output { source })?;

    print_hash(out, &expected)
}

/// Reads `plan.json` only. It was written by the `plan` step, whose hash the workflow compared
/// with the reviewed one before anything ran.
pub fn actions(args: &ActionsArgs, out: &mut impl Write) -> Result<(), Error> {
    let file: PlanFile = read_json(&args.plan)?;
    let action = args.action.into();

    file.programs
        .iter()
        .filter(|(_, program)| program.actions.contains(&action))
        .try_for_each(|(name, _)| {
            writeln!(out, "{name}").map_err(|source| Error::Output { source })
        })
}

fn rebuild(
    chain: &impl Chain,
    args: &ApplyArgs,
    deployer: &Pubkey,
) -> Result<(Plan, PlanHash), Error> {
    let file: PlanFile = read_json(&args.plan)?;
    let expected: PlanHash = file.hash.parse()?;
    let binaries = read_binaries(&file.document.release, &args.assets)?;
    let live = build(chain, &file.document, &binaries, deployer, &file.programs)?;

    match live.hash() == expected {
        true => Ok((live, expected)),
        false => Err(Error::PlanChanged {
            expected,
            actual: live.hash(),
        }),
    }
}

fn build(
    chain: &impl Chain,
    document: &PlanDocument,
    binaries: &Binaries,
    deployer: &Pubkey,
    planned: &BTreeMap<String, PlannedProgramFile>,
) -> Result<Plan, Error> {
    let PlanDocument { release, inputs } = document;
    let states = release
        .programs
        .iter()
        .map(|(name, program)| {
            let state = classify(chain, &program.address, &binaries[name], deployer)?;

            Ok((name.clone(), as_planned(state, planned.get(name))))
        })
        .collect::<Result<BTreeMap<_, _>, Error>>()?;
    let live_configs = config::read(chain, release)?;

    Ok(Plan::build(
        release.clone(),
        chain.genesis_hash()?,
        states,
        live_configs,
        Inputs::parse(inputs.clone())?,
    )?)
}

/// A program planned `New` that is now `Partial` (deployed from the release binary with this
/// deployer as authority) is what the `deploy` step produces; it stands for its planned `New`
/// state. Everything else is taken as classified.
fn as_planned(state: ProgramState, planned: Option<&PlannedProgramFile>) -> ProgramState {
    match (planned, &state.status) {
        (Some(planned), Status::Partial) if planned.status == PlannedStatus::New => ProgramState {
            address: state.address,
            status: Status::New,
            authority: None,
            data_hash: None,
        },
        _ => state,
    }
}

fn release(
    version: &str,
    cluster: Cluster,
    program_ids: &Path,
    assets: &Path,
) -> Result<(Release, Binaries), Error> {
    let ids: BTreeMap<String, ProgramId> = read_json(program_ids)?;
    let binaries = ids
        .keys()
        .map(|name| Ok((name.clone(), read(&asset_path(assets, name, cluster))?)))
        .collect::<Result<Binaries, Error>>()?;
    let programs = ids
        .into_iter()
        .map(|(name, ProgramId { address })| {
            let program = ReleaseProgram {
                address: address.parse().map_err(|_| Error::InvalidAddress {
                    program: name.clone(),
                    value: address,
                })?,
                so_sha256: sha256(&binaries[&name]),
            };

            Ok((name, program))
        })
        .collect::<Result<_, Error>>()?;
    let release = Release {
        version: version.into(),
        cluster,
        programs,
    };

    Ok((release, binaries))
}

/// The binaries must be the ones the plan hashed: classification compares them to the chain.
fn read_binaries(release: &Release, assets: &Path) -> Result<Binaries, Error> {
    release
        .programs
        .iter()
        .map(|(name, program)| {
            let path = asset_path(assets, name, release.cluster);
            let binary = read(&path)?;
            let actual = sha256(&binary);

            match actual == program.so_sha256 {
                true => Ok((name.clone(), binary)),
                false => Err(Error::AssetChanged {
                    program: name.clone(),
                    path,
                    expected: hex::encode(program.so_sha256),
                    actual: hex::encode(actual),
                }),
            }
        })
        .collect()
}

fn asset_path(assets: &Path, program: &str, cluster: Cluster) -> PathBuf {
    assets.join(format!("{program}.{cluster}.so"))
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    solana_sha256_hasher::hash(bytes).to_bytes()
}

fn read_keypair(path: &Path) -> Result<Keypair, Error> {
    read_keypair_file(path).map_err(|source| Error::Keypair {
        path: path.into(),
        source,
    })
}

fn read(path: &Path) -> Result<Vec<u8>, Error> {
    fs::read(path).map_err(|source| Error::Read {
        path: path.into(),
        source,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, Error> {
    serde_json::from_slice(&read(path)?).map_err(|source| Error::Json {
        path: path.into(),
        source,
    })
}

fn write_json(path: &Path, document: &PlanFile) -> Result<(), Error> {
    let json = serde_json::to_vec_pretty(document).expect("plan document must serialize");

    fs::write(path, json).map_err(|source| Error::Write {
        path: path.into(),
        source,
    })
}

fn append(path: &Path, text: &str) -> Result<(), Error> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|source| Error::Write {
            path: path.into(),
            source,
        })
}

fn print_step(out: &mut impl Write, step: &Step) -> Result<(), Error> {
    let signature = step
        .signature
        .map_or_else(|| "skipped".to_owned(), |signature| signature.to_string());

    writeln!(out, "{} {} {signature}", step.program, step.action)
        .and_then(|()| out.flush())
        .map_err(|source| Error::Output { source })
}

fn print_hash(out: &mut impl Write, hash: &PlanHash) -> Result<(), Error> {
    writeln!(out, "plan_hash={hash}").map_err(|source| Error::Output { source })
}

#[cfg(test)]
mod tests {
    use anchor_lang::AccountSerialize;
    use solana_loader_v3_interface::state::UpgradeableLoaderState;
    use solana_sdk::account::Account;
    use tempfile::TempDir;

    use super::*;
    use crate::cli::{ActionKind, ChainArgs, InputArgs};
    use crate::plan::{
        Action, AGGREGATOR_PROVER, HYPER_PROVER, LAYERZERO_PROVER, LOCAL_PROVER, POLYMER_PROVER,
    };
    use crate::testing::{release_address, RecordingChain, DVN, EXECUTOR, SENDER};

    const PROGRAMS: [&str; 5] = [
        AGGREGATOR_PROVER,
        HYPER_PROVER,
        LAYERZERO_PROVER,
        LOCAL_PROVER,
        POLYMER_PROVER,
    ];

    struct Env {
        directory: TempDir,
        deployer: Keypair,
    }

    impl Env {
        fn new() -> Self {
            let directory = TempDir::new().unwrap();
            let ids = PROGRAMS
                .iter()
                .map(|name| {
                    let entry = format!(
                        r#"{{"address":"{}","seed":"{}","salt":0}}"#,
                        release_address(name),
                        "00".repeat(32)
                    );

                    format!(r#""{name}":{entry}"#)
                })
                .collect::<Vec<_>>()
                .join(",");
            fs::write(directory.path().join("ids.json"), format!("{{{ids}}}")).unwrap();
            PROGRAMS.iter().for_each(|name| {
                fs::write(
                    directory.path().join(format!("{name}.devnet.so")),
                    binary(name),
                )
                .unwrap();
            });

            Self {
                directory,
                deployer: Keypair::new(),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.directory.path().join(name)
        }

        fn plan_args(&self) -> PlanArgs {
            let uln = format!(
                r#"{{"confirmations":15,"required_dvn_count":1,"optional_dvn_count":255,"optional_dvn_threshold":0,"required_dvns":["{DVN}"],"optional_dvns":[]}}"#
            );

            PlanArgs {
                version: "0.0.1".into(),
                cluster: Cluster::Devnet,
                program_ids: self.path("ids.json"),
                assets: self.directory.path().into(),
                out: self.path("plan.json"),
                summary: Some(self.path("summary.md")),
                chain: chain_args(),
                inputs: InputArgs {
                    hyper_senders: SENDER.into(),
                    polymer_emitters: SENDER.into(),
                    layerzero: format!(
                        r#"{{"peers":[{{"eid":1,"address":"{SENDER}","chain_id":1,"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}]}}"#
                    ),
                    hyper_reserve_lamports: "1000000".into(),
                    layerzero_reserve_lamports: "1000000".into(),
                    finalize_layerzero: false,
                },
            }
        }

        fn apply_args(&self) -> ApplyArgs {
            ApplyArgs {
                plan: self.path("plan.json"),
                assets: self.directory.path().into(),
                compute_unit_price: 0,
                chain: chain_args(),
            }
        }

        /// Runs `plan` against `chain` and returns the printed hash.
        fn plan(&self, chain: &RecordingChain) -> PlanHash {
            let mut out = Vec::new();
            plan(chain, &self.plan_args(), &self.deployer.pubkey(), &mut out).unwrap();

            hash_line(&out)
        }

        fn rebuild(&self, chain: &RecordingChain) -> Result<(Plan, PlanHash), Error> {
            rebuild(chain, &self.apply_args(), &self.deployer.pubkey())
        }

        fn apply(&self, chain: &mut RecordingChain) -> Error {
            apply(chain, &self.apply_args(), &self.deployer, &mut Vec::new()).unwrap_err()
        }

        fn actions(&self, action: ActionKind) -> Vec<String> {
            let args = ActionsArgs {
                plan: self.path("plan.json"),
                action,
            };
            let mut out = Vec::new();
            actions(&args, &mut out).unwrap();

            String::from_utf8(out)
                .unwrap()
                .lines()
                .map(Into::into)
                .collect()
        }

        fn deploy(&self, chain: &mut RecordingChain, name: &str) {
            self.deploy_as(chain, name, &binary(name), Some(self.deployer.pubkey()));
        }

        fn deploy_as(
            &self,
            chain: &mut RecordingChain,
            name: &str,
            elf: &[u8],
            authority: Option<Pubkey>,
        ) {
            let program = release_address(name);
            let programdata = Pubkey::find_program_address(&[b"data", name.as_bytes()], &program).0;
            let owner = solana_sdk_ids::bpf_loader_upgradeable::id();
            let mut data = bincode::serialize(&UpgradeableLoaderState::ProgramData {
                slot: 1,
                upgrade_authority_address: authority,
            })
            .unwrap();
            data.resize(UpgradeableLoaderState::size_of_programdata_metadata(), 0);
            data.extend_from_slice(elf);
            let program_account = bincode::serialize(&UpgradeableLoaderState::Program {
                programdata_address: programdata,
            })
            .unwrap();

            [(program, program_account), (programdata, data)]
                .into_iter()
                .for_each(|(address, data)| {
                    let account = Account {
                        owner,
                        data,
                        ..Account::default()
                    };
                    chain.accounts.insert(address, account);
                });
        }
    }

    fn binary(name: &str) -> Vec<u8> {
        format!("{name} bytecode").into_bytes()
    }

    fn chain_args() -> ChainArgs {
        ChainArgs {
            rpc_url: "unused".into(),
            deployer_keypair: "unused".into(),
        }
    }

    fn hash_line(out: &[u8]) -> PlanHash {
        String::from_utf8(out.to_vec())
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .strip_prefix("plan_hash=")
            .unwrap()
            .parse()
            .unwrap()
    }

    fn install_hyper_config(chain: &mut RecordingChain) {
        let owner = release_address(HYPER_PROVER);
        let mut data = Vec::new();
        hyper_prover::state::Config {
            whitelisted_senders: vec![(&SENDER.parse::<crate::EvmAddress>().unwrap()).into()],
        }
        .try_serialize(&mut data)
        .unwrap();
        chain.accounts.insert(
            Pubkey::find_program_address(&[hyper_prover::state::CONFIG_SEED], &owner).0,
            Account {
                owner,
                data,
                ..Account::default()
            },
        );
    }

    #[test]
    fn plan_writes_the_full_plan_summary_and_hash() {
        let env = Env::new();
        let hash = env.plan(&RecordingChain::default());

        let file: PlanFile = read_json(&env.path("plan.json")).unwrap();
        let summary = fs::read_to_string(env.path("summary.md")).unwrap();

        assert_eq!(file.hash, hash.to_string());
        assert_eq!(file.document.release.programs.len(), PROGRAMS.len());
        assert_eq!(file.programs[HYPER_PROVER].status, PlannedStatus::New);
        assert_eq!(
            file.programs[HYPER_PROVER].actions,
            [
                Action::Deploy,
                Action::Init,
                Action::Verify,
                Action::Finalize
            ]
        );
        assert!(
            summary.ends_with(&format!("Plan hash: {hash}\n")),
            "{summary}"
        );
    }

    #[test]
    fn plan_fails_on_a_foreign_program() {
        let env = Env::new();
        let mut chain = RecordingChain::default();
        chain
            .accounts
            .insert(release_address(LOCAL_PROVER), Account::default());

        let error = plan(
            &chain,
            &env.plan_args(),
            &env.deployer.pubkey(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(
            matches!(error, Error::Plan(plan::Error::Foreign { .. })),
            "{error:?}"
        );
        assert!(!env.path("plan.json").exists());
    }

    #[test]
    fn plan_fails_on_an_rpc_of_another_cluster() {
        let env = Env::new();
        let chain = RecordingChain {
            genesis_hash: Cluster::Mainnet.genesis_hash(),
            ..RecordingChain::default()
        };

        let error = plan(
            &chain,
            &env.plan_args(),
            &env.deployer.pubkey(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(
            matches!(error, Error::Plan(plan::Error::ClusterMismatch { .. })),
            "{error:?}"
        );
        assert!(!env.path("plan.json").exists());
    }

    #[test]
    fn plan_fails_on_a_missing_binary() {
        let env = Env::new();
        fs::remove_file(env.path("hyper_prover.devnet.so")).unwrap();

        let error = plan(
            &RecordingChain::default(),
            &env.plan_args(),
            &env.deployer.pubkey(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(matches!(error, Error::Read { .. }), "{error:?}");
    }

    #[test]
    fn actions_lists_programs_for_action_from_the_plan_file_alone() {
        let env = Env::new();
        env.plan(&RecordingChain::default());
        fs::remove_file(env.path("ids.json")).unwrap();
        PROGRAMS
            .iter()
            .for_each(|name| fs::remove_file(env.path(&format!("{name}.devnet.so"))).unwrap());

        assert_eq!(env.actions(ActionKind::Deploy), PROGRAMS);
        assert_eq!(env.actions(ActionKind::Verify), PROGRAMS);
        assert_eq!(
            env.actions(ActionKind::Finalize),
            [
                AGGREGATOR_PROVER,
                HYPER_PROVER,
                LOCAL_PROVER,
                POLYMER_PROVER
            ]
        );
    }

    #[test]
    fn rebuild_accepts_the_chain_unchanged() {
        let env = Env::new();
        let chain = RecordingChain::default();
        let hash = env.plan(&chain);

        assert_eq!(env.rebuild(&chain).unwrap().1, hash);
    }

    #[test]
    fn rebuild_accepts_planned_new_programs_after_the_deploy_step() {
        let env = Env::new();
        let mut chain = RecordingChain::default();
        let hash = env.plan(&chain);
        PROGRAMS
            .iter()
            .for_each(|name| env.deploy(&mut chain, name));

        let (live, expected) = env.rebuild(&chain).unwrap();

        assert_eq!(expected, hash);
        assert_eq!(live.hash(), hash);
    }

    #[test]
    fn apply_rejects_changed_plan() {
        let env = Env::new();
        let mut chain = RecordingChain::default();
        let hash = env.plan(&chain);
        env.deploy(&mut chain, HYPER_PROVER);
        install_hyper_config(&mut chain);

        let error = env.apply(&mut chain);

        assert!(
            matches!(error, Error::PlanChanged { expected, actual } if expected == hash && actual != hash),
            "{error:?}"
        );
        assert!(chain.sent.is_empty());
    }

    #[test]
    fn apply_rejects_a_planned_partial_program_that_changed() {
        let env = Env::new();
        let mut with_config = RecordingChain::default();
        env.deploy(&mut with_config, HYPER_PROVER);
        let hash = env.plan(&with_config);
        install_hyper_config(&mut with_config);

        let config_appeared = env.apply(&mut with_config);
        let mut with_other_authority = RecordingChain::default();
        env.deploy(&mut with_other_authority, HYPER_PROVER);
        env.deploy_as(
            &mut with_other_authority,
            HYPER_PROVER,
            &binary(HYPER_PROVER),
            Some(Pubkey::new_unique()),
        );
        let authority_changed = env.apply(&mut with_other_authority);

        assert!(
            matches!(config_appeared, Error::PlanChanged { expected, .. } if expected == hash),
            "{config_appeared:?}"
        );
        assert!(
            matches!(authority_changed, Error::Plan(plan::Error::Foreign { .. })),
            "{authority_changed:?}"
        );
        assert!(with_config.sent.is_empty());
        assert!(with_other_authority.sent.is_empty());
    }

    #[test]
    fn apply_rejects_a_planned_live_program_that_changed() {
        let env = Env::new();
        let mut chain = RecordingChain::default();
        env.deploy_as(&mut chain, LOCAL_PROVER, &binary(LOCAL_PROVER), None);
        env.plan(&chain);
        env.deploy_as(&mut chain, LOCAL_PROVER, b"upgraded", None);

        let error = env.apply(&mut chain);

        assert!(
            matches!(error, Error::Plan(plan::Error::Foreign { .. })),
            "{error:?}"
        );
    }

    #[test]
    fn apply_rejects_a_planned_partial_program_that_is_now_live() {
        let env = Env::new();
        let mut chain = RecordingChain::default();
        env.deploy(&mut chain, LOCAL_PROVER);
        env.plan(&chain);
        env.deploy_as(&mut chain, LOCAL_PROVER, &binary(LOCAL_PROVER), None);

        let error = env.apply(&mut chain);

        assert!(matches!(error, Error::PlanChanged { .. }), "{error:?}");
        assert!(chain.sent.is_empty());
    }

    #[test]
    fn apply_rejects_a_planned_new_program_deployed_by_someone_else() {
        let env = Env::new();
        let mut chain = RecordingChain::default();
        env.plan(&chain);
        env.deploy_as(
            &mut chain,
            HYPER_PROVER,
            &binary(HYPER_PROVER),
            Some(Pubkey::new_unique()),
        );

        let error = env.apply(&mut chain);

        assert!(
            matches!(error, Error::Plan(plan::Error::Foreign { .. })),
            "{error:?}"
        );
        assert!(chain.sent.is_empty());
    }

    #[test]
    fn apply_rejects_a_replaced_binary() {
        let env = Env::new();
        let mut chain = RecordingChain::default();
        env.plan(&chain);
        fs::write(env.path("hyper_prover.devnet.so"), b"another build").unwrap();

        let error = env.apply(&mut chain);

        assert!(
            matches!(error, Error::AssetChanged { ref program, .. } if program == HYPER_PROVER),
            "{error:?}"
        );
    }
}
