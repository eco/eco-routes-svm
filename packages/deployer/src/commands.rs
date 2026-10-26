use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Keypair};
use solana_sdk::signer::Signer;

use crate::apply::{self, Step};
use crate::chain::{self, Chain};
use crate::classify::{self, classify, ProgramState, Status};
use crate::cli::{ActionsArgs, ApplyArgs, CloseIdlArgs, Command, PlanArgs, SelectionArgs};
use crate::funding::Funding;
use crate::idl::{self, Outcome};
use crate::inputs::{self, Inputs, RawInputs};
use crate::plan::{
    self, Action, Cluster, Plan, PlanDocument, PlanFile, PlanHash, PlannedProgramFile,
    PlannedStatus, Release, ReleaseProgram,
};
use crate::rpc::RpcChain;
use crate::selection::{self, Operation, Programs, Selection};
use crate::{config, funding, layerzero, readback};

/// Wait before rereading the IDL accounts after a failed transaction, times the number of
/// failures in a row.
const IDL_RETRY_BACKOFF: Duration = Duration::from_secs(2);

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
    Funding(#[from] funding::Error),
    #[error(transparent)]
    Idl(#[from] idl::Error),
    #[error(transparent)]
    Inputs(#[from] inputs::Error),
    #[error(transparent)]
    Plan(#[from] plan::Error),
    #[error(transparent)]
    Readback(#[from] readback::Error),
    #[error(transparent)]
    Selection(#[from] selection::Error),
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
    #[error("{program}: {path} is for {actual:?}, not {expected}")]
    IdlAddress {
        program: String,
        path: PathBuf,
        expected: Pubkey,
        actual: Option<String>,
    },
    #[error("{program}: {address} is {status}, not a release program the deployer can still upgrade, so it cannot write its IDL accounts")]
    IdlNotWritable {
        program: String,
        address: Pubkey,
        status: String,
    },
    #[error("{program}: {value:?} in program-ids.json is not an address")]
    InvalidAddress { program: String, value: String },
    #[error("{program}: dependency {dependency:?} in program-ids.json is not a released program")]
    UnknownDependency { program: String, dependency: String },
    /// Never carries the parser's error: it can quote the file, which holds the secret key.
    #[error("cannot read deployer keypair {path}: {reason}")]
    Keypair { path: PathBuf, reason: &'static str },
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

/// The fields of either plan file that `actions` needs.
#[derive(Deserialize)]
struct ActionsFile {
    release: Release,
    programs: BTreeMap<String, PlannedProgramFile>,
}

/// The `program-ids.json` entry fields the plan needs; `seed` and `salt` only matter to
/// `scripts/program-keypairs.mjs`.
#[derive(Deserialize)]
struct ProgramId {
    address: String,
    dependencies: Vec<String>,
}

type Binaries = BTreeMap<String, Vec<u8>>;

pub fn run(command: &Command, out: &mut impl Write) -> Result<(), Error> {
    match command {
        Command::Plan(args) => {
            let chain = RpcChain::new(args.chain.rpc_url.clone());

            plan(&chain, args, &args.chain.deployer, out)
        }
        Command::Apply(args) => {
            let deployer = read_keypair(&args.chain.deployer_keypair)?;
            let file: PlanFile = read_json(&args.plan)?;
            let price = Inputs::parse(file.document.inputs)?.compute_unit_price;
            let mut chain =
                RpcChain::new(args.chain.rpc_url.clone()).with_compute_unit_price(price);

            apply(&mut chain, args, &deployer, out)
        }
        Command::PlanFinalize(args) => {
            let chain = RpcChain::new(args.chain.rpc_url.clone());

            plan_selection(&chain, Operation::Finalize, args, &args.chain.deployer, out)
        }
        Command::PlanClose(args) => {
            let chain = RpcChain::new(args.chain.rpc_url.clone());

            plan_selection(&chain, Operation::Close, args, &args.chain.deployer, out)
        }
        Command::Actions(args) => actions(args, out),
        Command::Idl(args) => {
            let deployer = read_keypair(&args.chain.deployer_keypair)?;
            let PlanFile { document, .. } = read_json(&args.plan)?;
            let price = Inputs::parse(document.inputs)?.compute_unit_price;
            let mut chain =
                RpcChain::new(args.chain.rpc_url.clone()).with_compute_unit_price(price);

            idl(
                &mut chain,
                &document.release,
                &args.assets,
                &deployer,
                IDL_RETRY_BACKOFF,
                out,
            )
        }
        Command::CloseIdl(args) => {
            let deployer = read_keypair(&args.chain.deployer_keypair)?;
            let mut chain = RpcChain::new(args.chain.rpc_url.clone());

            close_idl(&mut chain, args, &deployer, IDL_RETRY_BACKOFF, out)
        }
    }
}

/// Writes `plan.json` and the summary and prints the hash even when the deployer is short of
/// funds, so the reviewer sees the whole plan, then fails.
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
        cctp: inputs.cctp.clone(),
        hyper_reserve_lamports: inputs.hyper_reserve_lamports.clone(),
        layerzero_reserve_lamports: inputs.layerzero_reserve_lamports.clone(),
        compute_unit_price: inputs.compute_unit_price.clone(),
    };
    let document = PlanDocument {
        release,
        inputs: raw.clone(),
    };
    let plan = build(chain, &document, &binaries, deployer, &BTreeMap::new())?;
    let idls = read_idls(&plan.release, assets)?;
    let funding = Funding::estimate(chain, &plan, &binaries, &idls, deployer)?;

    write_json(plan_path, &plan.file(raw))?;
    summary
        .iter()
        .try_for_each(|path| append(path, &format!("{}\n{funding}", plan.summary())))?;
    print_hash(out, &plan.hash())?;

    Ok(funding.require()?)
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
    let landed = apply::apply(chain, &live, deployer, &mut |step| {
        printed = mem::replace(&mut printed, Ok(())).and_then(|()| print_step(out, step));
    });
    printed?;
    landed?;
    let read_back = readback::readback(chain, &live)?;
    write!(out, "read back:\n{read_back}").map_err(|source| Error::Output { source })?;

    print_hash(out, &expected)
}

/// Writes `plan.json` for finalizing or closing the selected programs and prints its hash.
pub fn plan_selection(
    chain: &impl Chain,
    operation: Operation,
    args: &SelectionArgs,
    deployer: &Pubkey,
    out: &mut impl Write,
) -> Result<(), Error> {
    let SelectionArgs {
        version,
        cluster,
        program_ids,
        assets,
        out: plan_path,
        summary,
        programs,
        ..
    } = args;
    let (release, binaries) = release(version, *cluster, program_ids, assets)?;
    let selected: Programs = programs.parse()?;
    let idls = read_idls(&release, assets)?;
    let observed = selection::observe(chain, &release, &binaries, &idls, deployer)?;
    let selection = Selection::build(operation, &selected, release, *deployer, observed)?;

    write_json(plan_path, &selection.file(programs.clone()))?;
    summary
        .iter()
        .try_for_each(|path| append(path, &selection.summary()))?;

    print_hash(out, &selection.hash())
}

/// Reads only `plan.json`, from any plan kind. It was written by the plan step, whose hash the
/// workflow compared with the reviewed one before anything ran. A program comes after its
/// dependencies, or before them for `close`, so a run that stops part-way never leaves a final
/// program depending on an upgradeable one, nor a program depending on a closed one.
pub fn actions(args: &ActionsArgs, out: &mut impl Write) -> Result<(), Error> {
    let ActionsFile { release, programs } = read_json(&args.plan)?;
    let action = args.action.into();
    let dependencies_first = release.dependency_order();
    let order = match action {
        Action::Close => dependencies_first.into_iter().rev().collect(),
        _ => dependencies_first,
    };

    order
        .into_iter()
        .filter(|name| {
            programs
                .get(*name)
                .is_some_and(|program| program.actions.contains(&action))
        })
        .try_for_each(|name| writeln!(out, "{name}").map_err(|source| Error::Output { source }))
}

/// Uploads each release program's IDL from `assets` as its canonical metadata, dependencies
/// first. Which programs it writes to comes from the chain: those still upgradeable by the
/// deployer with the release bytecode (`Partial`). A final one (`Live`) can no longer change its
/// IDL, so it is reported and left alone; anything else is refused. Stops at the first failure;
/// a rerun skips what already matches and resumes a half-written upload.
pub fn idl(
    chain: &mut impl Chain,
    release: &Release,
    assets: &Path,
    deployer: &Keypair,
    backoff: Duration,
    out: &mut impl Write,
) -> Result<(), Error> {
    release.dependency_order().into_iter().try_for_each(|name| {
        let address = release.programs[name].address;
        let path = idl_path(assets, name, release.cluster);
        let idl = read_idl(&path, name, &address)?;
        let canonical = idl::canonical_address(&address);

        match upgradeable(chain, release, assets, name, deployer)? {
            false => {
                let state = match idl::matches(chain.account(&canonical)?.as_ref(), &address, &idl)
                {
                    true => "matches the release",
                    false => "differs from the release and can no longer change",
                };
                line(
                    out,
                    format!("{name} idl final, left as is: {canonical} {state}"),
                )
            }
            true => {
                let mut printed = Ok(());
                let outcome = idl::upload(
                    chain,
                    name,
                    &address,
                    &programdata_of(&address),
                    deployer,
                    &idl,
                    backoff,
                    &mut |step, signature| {
                        printed = mem::replace(&mut printed, Ok(()))
                            .and_then(|()| line(out, format!("{name} idl {step} {signature}")));
                    },
                );
                printed?;
                let outcome = match outcome? {
                    Outcome::Unchanged => "unchanged",
                    Outcome::Uploaded => "uploaded",
                };
                line(
                    out,
                    format!(
                        "{name} idl {outcome}: {canonical} holds {} ({} bytes)",
                        path.display(),
                        idl.len()
                    ),
                )
            }
        }
    })
}

/// Closes the IDL accounts of every program the close plan closes, dependents first, returning
/// their rent to the deployer. It runs before `solana program close`: once a program's data is
/// gone nobody can authorize closing them, and their rent is locked for good.
pub fn close_idl(
    chain: &mut impl Chain,
    args: &CloseIdlArgs,
    deployer: &Keypair,
    backoff: Duration,
    out: &mut impl Write,
) -> Result<(), Error> {
    let ActionsFile { release, programs } = read_json(&args.plan)?;

    release
        .dependency_order()
        .into_iter()
        .rev()
        .filter(|name| {
            programs
                .get(*name)
                .is_some_and(|program| program.actions.contains(&Action::Close))
        })
        .try_for_each(|name| {
            let address = release.programs[name].address;
            if !upgradeable(chain, &release, &args.assets, name, deployer)? {
                return line(out, format!("{name} idl left: the program is final"));
            }
            let [canonical, scratch] = idl::held_lamports(chain, &address)?;
            let mut printed = Ok(());
            let outcome = idl::close(
                chain,
                name,
                &address,
                &programdata_of(&address),
                deployer,
                backoff,
                &mut |step, signature| {
                    printed = mem::replace(&mut printed, Ok(()))
                        .and_then(|()| line(out, format!("{name} idl {step} {signature}")));
                },
            );
            printed?;

            line(
                out,
                match outcome? {
                    Outcome::Unchanged => format!("{name} idl: nothing to close"),
                    Outcome::Uploaded => format!(
                        "{name} idl closed: {} lamports back to the deployer",
                        canonical + scratch
                    ),
                },
            )
        })
}

/// Whether the deployer can still upgrade `name`: deployed from the release bytecode with the
/// deployer as its authority. `false` for a final one; any other state is refused.
fn upgradeable(
    chain: &impl Chain,
    release: &Release,
    assets: &Path,
    name: &str,
    deployer: &Keypair,
) -> Result<bool, Error> {
    let address = release.programs[name].address;
    let binary = read(&asset_path(assets, name, release.cluster))?;

    match classify(chain, &address, &binary, &deployer.pubkey())?.status {
        Status::Partial => Ok(true),
        Status::Live => Ok(false),
        status => Err(Error::IdlNotWritable {
            program: name.into(),
            address,
            status: match status {
                Status::Foreign { reason } => format!("foreign ({reason})"),
                other => PlannedStatus::from(&other).to_string(),
            },
        }),
    }
}

fn programdata_of(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[program.as_ref()],
        &solana_sdk_ids::bpf_loader_upgradeable::id(),
    )
    .0
}

fn line(out: &mut impl Write, text: String) -> Result<(), Error> {
    writeln!(out, "{text}")
        .and_then(|()| out.flush())
        .map_err(|source| Error::Output { source })
}

/// The release IDL for `program`, refused unless it names `address`: the canonical account
/// would otherwise serve another program's interface.
fn read_idl(path: &Path, program: &str, address: &Pubkey) -> Result<Vec<u8>, Error> {
    #[derive(Deserialize)]
    struct Named {
        address: Option<String>,
    }

    let idl = read(path)?;
    let Named { address: named } = serde_json::from_slice(&idl).map_err(|source| Error::Json {
        path: path.into(),
        source,
    })?;

    match named.as_deref() == Some(address.to_string().as_str()) {
        true => Ok(idl),
        false => Err(Error::IdlAddress {
            program: program.into(),
            path: path.into(),
            expected: *address,
            actual: named,
        }),
    }
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
    let built = Plan::build(
        release.clone(),
        chain.genesis_hash()?,
        *deployer,
        states,
        live_configs,
        Inputs::parse(inputs.clone())?,
    )?;
    // After `Plan::build`, so a wrong cluster is reported as that, not as a missing worker.
    layerzero::check_workers(chain, &built).map_err(plan::Error::from)?;

    Ok(built)
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
        .map(
            |(
                name,
                ProgramId {
                    address,
                    dependencies,
                },
            )| {
                if let Some(unknown) = dependencies
                    .iter()
                    .find(|dependency| !binaries.contains_key(*dependency))
                {
                    return Err(Error::UnknownDependency {
                        program: name,
                        dependency: unknown.clone(),
                    });
                }
                let program = ReleaseProgram {
                    address: address.parse().map_err(|_| Error::InvalidAddress {
                        program: name.clone(),
                        value: address,
                    })?,
                    so_sha256: sha256(&binaries[&name]),
                    dependencies,
                };

                Ok((name, program))
            },
        )
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

fn idl_path(assets: &Path, program: &str, cluster: Cluster) -> PathBuf {
    assets.join(format!("{program}.{cluster}.json"))
}

fn read_idls(release: &Release, assets: &Path) -> Result<BTreeMap<String, Vec<u8>>, Error> {
    release
        .programs
        .keys()
        .map(|name| {
            Ok((
                name.clone(),
                read(&idl_path(assets, name, release.cluster))?,
            ))
        })
        .collect()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    solana_sha256_hasher::hash(bytes).to_bytes()
}

fn read_keypair(path: &Path) -> Result<Keypair, Error> {
    read_keypair_file(path).map_err(|_| Error::Keypair {
        path: path.into(),
        reason: match path.exists() {
            true => "not a JSON array of 64 bytes",
            false => "no such file",
        },
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

fn write_json(path: &Path, document: &impl Serialize) -> Result<(), Error> {
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
    use crate::cli::{ActionKind, ChainArgs, InputArgs, ReadArgs};
    use crate::plan::{
        Action, AGGREGATOR_PROVER, CCTP_PROVER, HYPER_PROVER, LAYERZERO_PROVER, LOCAL_PROVER,
        POLYMER_PROVER,
    };
    use crate::selection::SelectionFile;
    use crate::testing::{
        self, dependencies, full_setup, release_address, RecordingChain, DVN, EXECUTOR, SENDER,
    };

    const VERIFY_PROGRAM_ID: Pubkey =
        Pubkey::from_str_const("verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC");
    const PROGRAMS: [&str; 6] = [
        AGGREGATOR_PROVER,
        CCTP_PROVER,
        HYPER_PROVER,
        LAYERZERO_PROVER,
        LOCAL_PROVER,
        POLYMER_PROVER,
    ];

    const DEPLOYER_LAMPORTS: u64 = 1_000_000_000;

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
                        r#"{{"address":"{}","seed":"{}","salt":0,"dependencies":{}}}"#,
                        release_address(name),
                        "00".repeat(32),
                        serde_json::to_string(&dependencies(name)).unwrap()
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
                fs::write(
                    directory.path().join(format!("{name}.devnet.json")),
                    release_idl(name),
                )
                .unwrap();
            });

            Self {
                directory,
                deployer: Keypair::new(),
            }
        }

        /// A chain holding the deployer's balance and the fixture's LayerZero worker accounts.
        fn chain(&self) -> RecordingChain {
            let mut chain = RecordingChain::default();
            chain.accounts.insert(
                self.deployer.pubkey(),
                Account {
                    lamports: DEPLOYER_LAMPORTS,
                    ..Account::default()
                },
            );

            chain
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
                chain: self.read_args(),
                inputs: InputArgs {
                    hyper_senders: SENDER.into(),
                    polymer_emitters: SENDER.into(),
                    layerzero: format!(
                        r#"{{"peers":[{{"eid":1,"address":"{SENDER}","chain_id":1,"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}]}}"#
                    ),
                    cctp: format!(
                        r#"{{"peers":[{{"domain":0,"chain_id":1,"sender":"{SENDER}"}}]}}"#
                    ),
                    hyper_reserve_lamports: "1000000".into(),
                    layerzero_reserve_lamports: "100000000".into(),
                    compute_unit_price: "0".into(),
                },
            }
        }

        /// Runs `plan-finalize` or `plan-close` for `programs` against `chain`.
        fn plan_selection(
            &self,
            chain: &RecordingChain,
            operation: Operation,
            programs: &str,
        ) -> Result<PlanHash, Error> {
            let args = SelectionArgs {
                version: "0.0.1".into(),
                cluster: Cluster::Devnet,
                program_ids: self.path("ids.json"),
                assets: self.directory.path().into(),
                out: self.path("plan.json"),
                summary: Some(self.path("summary.md")),
                programs: programs.into(),
                chain: self.read_args(),
            };
            let mut out = Vec::new();
            plan_selection(chain, operation, &args, &self.deployer.pubkey(), &mut out)?;

            Ok(hash_line(&out))
        }

        /// Every program deployed by this deployer, initialized, its LayerZero setup complete,
        /// its IDL uploaded, and verified unless named in `unverified`.
        fn tested_chain(&self, unverified: &[&str]) -> RecordingChain {
            let mut chain = full_setup(&testing::plan(5_000_000)).chain;
            PROGRAMS.iter().for_each(|name| {
                self.deploy(&mut chain, name);
                let program = release_address(name);
                chain.accounts.insert(
                    idl::canonical_address(&program),
                    idl::uploaded_account(&program, &release_idl(name)),
                );
                if unverified.contains(name) {
                    return;
                }
                let record =
                    selection::verification_record(&self.deployer.pubkey(), &release_address(name));
                chain.accounts.insert(
                    record,
                    Account {
                        owner: VERIFY_PROGRAM_ID,
                        data: vec![1],
                        ..Account::default()
                    },
                );
            });

            chain
        }

        fn read_args(&self) -> ReadArgs {
            ReadArgs {
                rpc_url: "unused".into(),
                deployer: self.deployer.pubkey(),
            }
        }

        fn apply_args(&self) -> ApplyArgs {
            ApplyArgs {
                plan: self.path("plan.json"),
                assets: self.directory.path().into(),
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

        /// As `solana program close` leaves it: the program account without its programdata.
        fn close(&self, chain: &mut RecordingChain, name: &str) {
            chain.accounts.remove(&programdata_address(name));
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
            let programdata = programdata_address(name);
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

    /// Not the loader's PDA: classification follows the program account's pointer.
    fn programdata_address(name: &str) -> Pubkey {
        Pubkey::find_program_address(&[b"data", name.as_bytes()], &release_address(name)).0
    }

    fn binary(name: &str) -> Vec<u8> {
        format!("{name} bytecode").into_bytes()
    }

    fn release_idl(name: &str) -> Vec<u8> {
        format!(
            r#"{{"address":"{}","metadata":{{"name":"{name}","version":"0.0.1"}}}}"#,
            release_address(name)
        )
        .into_bytes()
    }

    impl Env {
        /// Runs `idl` for the release in `plan.json` against `chain` and returns what it printed.
        fn idl(&self, chain: &mut RecordingChain) -> Result<String, Error> {
            let ActionsFile { release, .. } = read_json(&self.path("plan.json"))?;
            let mut out = Vec::new();
            idl(
                chain,
                &release,
                self.directory.path(),
                &self.deployer,
                Duration::ZERO,
                &mut out,
            )?;

            Ok(String::from_utf8(out).unwrap())
        }

        /// Runs `close-idl` for the close plan in `plan.json` and returns what it printed.
        fn close_idl(&self, chain: &mut RecordingChain) -> Result<String, Error> {
            let args = CloseIdlArgs {
                plan: self.path("plan.json"),
                assets: self.directory.path().into(),
                chain: chain_args(),
            };
            let mut out = Vec::new();
            close_idl(chain, &args, &self.deployer, Duration::ZERO, &mut out)?;

            Ok(String::from_utf8(out).unwrap())
        }
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
        let hash = env.plan(&env.chain());

        let file: PlanFile = read_json(&env.path("plan.json")).unwrap();
        let summary = fs::read_to_string(env.path("summary.md")).unwrap();

        assert_eq!(file.hash, hash.to_string());
        assert_eq!(file.document.release.programs.len(), PROGRAMS.len());
        assert_eq!(file.programs[HYPER_PROVER].status, PlannedStatus::New);
        assert_eq!(
            file.programs[HYPER_PROVER].actions,
            [Action::Deploy, Action::Init, Action::Verify]
        );
        assert!(
            summary.contains(&format!("Plan hash: {hash}\n\n### Deployer funding")),
            "{summary}"
        );
        assert!(summary.ends_with("Funded.\n"), "{summary}");
    }

    #[test]
    fn plan_records_the_plan_before_failing_on_an_underfunded_deployer() {
        let env = Env::new();
        let mut out = Vec::new();

        let error = plan(
            &RecordingChain::default(),
            &env.plan_args(),
            &env.deployer.pubkey(),
            &mut out,
        )
        .unwrap_err();

        let summary = fs::read_to_string(env.path("summary.md")).unwrap();
        let file: PlanFile = read_json(&env.path("plan.json")).unwrap();
        assert!(
            matches!(error, Error::Funding(funding::Error::InsufficientBalance { deployer, .. }) if deployer == env.deployer.pubkey()),
            "{error:?}"
        );
        assert_eq!(file.hash, hash_line(&out).to_string());
        assert!(summary.contains("**Short by "), "{summary}");
    }

    #[test]
    fn plan_fails_on_a_foreign_program() {
        let env = Env::new();
        let mut chain = env.chain();
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
            ..env.chain()
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
    fn plan_fails_on_an_executor_program_id_and_writes_nothing() {
        let env = Env::new();
        let mut chain = env.chain();
        let executor = EXECUTOR.parse().unwrap();
        chain.accounts.get_mut(&executor).unwrap().executable = true;

        let error = plan(
            &chain,
            &env.plan_args(),
            &env.deployer.pubkey(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(
            matches!(
                error,
                Error::Plan(plan::Error::LayerZero(layerzero::Error::WorkerIsProgram {
                    role: layerzero::Role::Executor,
                    ..
                }))
            ),
            "{error:?}"
        );
        assert!(!env.path("plan.json").exists());
    }

    #[test]
    fn plan_fails_on_a_missing_binary() {
        let env = Env::new();
        fs::remove_file(env.path("hyper_prover.devnet.so")).unwrap();

        let error = plan(
            &env.chain(),
            &env.plan_args(),
            &env.deployer.pubkey(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(matches!(error, Error::Read { .. }), "{error:?}");
    }

    #[test]
    fn actions_lists_programs_from_the_plan_file_alone_dependencies_first() {
        let env = Env::new();
        env.plan(&env.chain());
        fs::remove_file(env.path("ids.json")).unwrap();
        PROGRAMS
            .iter()
            .for_each(|name| fs::remove_file(env.path(&format!("{name}.devnet.so"))).unwrap());

        let members_then_aggregator = [
            CCTP_PROVER,
            HYPER_PROVER,
            LAYERZERO_PROVER,
            LOCAL_PROVER,
            POLYMER_PROVER,
            AGGREGATOR_PROVER,
        ];
        assert_eq!(env.actions(ActionKind::Deploy), members_then_aggregator);
        assert_eq!(env.actions(ActionKind::Verify), members_then_aggregator);
        assert!(env.actions(ActionKind::Finalize).is_empty());
    }

    #[test]
    fn rebuild_accepts_the_chain_unchanged() {
        let env = Env::new();
        let chain = env.chain();
        let hash = env.plan(&chain);

        assert_eq!(env.rebuild(&chain).unwrap().1, hash);
    }

    #[test]
    fn rebuild_accepts_planned_new_programs_after_the_deploy_step() {
        let env = Env::new();
        let mut chain = env.chain();
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
        let mut chain = env.chain();
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
        let mut with_config = env.chain();
        env.deploy(&mut with_config, HYPER_PROVER);
        let hash = env.plan(&with_config);
        install_hyper_config(&mut with_config);

        let config_appeared = env.apply(&mut with_config);
        let mut with_other_authority = env.chain();
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
        let mut chain = env.chain();
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
        let mut chain = env.chain();
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
        let mut chain = env.chain();
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
        let mut chain = env.chain();
        env.plan(&chain);
        fs::write(env.path("hyper_prover.devnet.so"), b"another build").unwrap();

        let error = env.apply(&mut chain);

        assert!(
            matches!(error, Error::AssetChanged { ref program, .. } if program == HYPER_PROVER),
            "{error:?}"
        );
    }

    #[test]
    fn plan_finalize_lists_tested_programs_dependencies_first() {
        let env = Env::new();
        let chain = env.tested_chain(&[]);

        let hash = env
            .plan_selection(&chain, Operation::Finalize, "all")
            .unwrap();

        let file: SelectionFile = read_json(&env.path("plan.json")).unwrap();
        let summary = fs::read_to_string(env.path("summary.md")).unwrap();
        assert_eq!(file.hash, hash.to_string());
        assert_eq!(file.selected, "all");
        assert_eq!(
            env.actions(ActionKind::Finalize),
            [
                CCTP_PROVER,
                HYPER_PROVER,
                LAYERZERO_PROVER,
                LOCAL_PROVER,
                POLYMER_PROVER,
                AGGREGATOR_PROVER
            ]
        );
        assert!(env.actions(ActionKind::Close).is_empty());
        assert!(
            summary.ends_with(&format!("Plan hash: {hash}\n")),
            "{summary}"
        );
    }

    #[test]
    fn plan_finalize_refuses_an_unverified_program_and_writes_nothing() {
        let env = Env::new();
        let chain = env.tested_chain(&[POLYMER_PROVER]);

        let error = env
            .plan_selection(&chain, Operation::Finalize, POLYMER_PROVER)
            .unwrap_err();

        assert!(
            matches!(&error, Error::Selection(selection::Error::NotVerified { program }) if program == POLYMER_PROVER),
            "{error:?}"
        );
        assert!(!env.path("plan.json").exists());
    }

    #[test]
    fn plan_finalize_refuses_a_program_whose_canonical_idl_differs() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        let program = release_address(POLYMER_PROVER);
        chain.accounts.insert(
            idl::canonical_address(&program),
            idl::uploaded_account(&program, br#"{"address":"older"}"#),
        );

        let error = env
            .plan_selection(&chain, Operation::Finalize, POLYMER_PROVER)
            .unwrap_err();

        assert!(
            matches!(&error, Error::Selection(selection::Error::IdlNotUploaded { program }) if program == POLYMER_PROVER),
            "{error:?}"
        );
    }

    #[test]
    fn idl_sends_nothing_when_every_canonical_idl_already_matches() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        env.plan_selection(&chain, Operation::Close, "all").unwrap();

        let printed = env.idl(&mut chain).unwrap();

        assert!(chain.sent.is_empty(), "{:?}", chain.sent);
        let unchanged: Vec<&str> = printed
            .lines()
            .map(|line| line.split(' ').next().unwrap())
            .collect();
        assert_eq!(
            unchanged,
            [
                CCTP_PROVER,
                HYPER_PROVER,
                LAYERZERO_PROVER,
                LOCAL_PROVER,
                POLYMER_PROVER,
                AGGREGATOR_PROVER
            ]
        );
        assert!(
            printed
                .lines()
                .all(|line| line.contains(" idl unchanged: ")),
            "{printed}"
        );
    }

    #[test]
    fn idl_reports_a_final_program_and_leaves_it_alone() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        env.plan_selection(&chain, Operation::Close, "all").unwrap();
        env.deploy_as(&mut chain, HYPER_PROVER, &binary(HYPER_PROVER), None);
        chain
            .accounts
            .remove(&idl::canonical_address(&release_address(HYPER_PROVER)));

        let printed = env.idl(&mut chain).unwrap();

        assert!(chain.sent.is_empty(), "{:?}", chain.sent);
        assert!(
            printed.contains("hyper_prover idl final, left as is: ")
                && printed.contains("differs from the release and can no longer change"),
            "{printed}"
        );
    }

    #[test]
    fn idl_refuses_another_authority_and_an_idl_for_another_address() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        env.plan_selection(&chain, Operation::Close, "all").unwrap();
        let other = Pubkey::new_unique();
        env.deploy_as(&mut chain, HYPER_PROVER, &binary(HYPER_PROVER), Some(other));

        let foreign = env.idl(&mut chain).unwrap_err();
        env.deploy(&mut chain, HYPER_PROVER);
        fs::write(
            env.path("hyper_prover.devnet.json"),
            release_idl(LOCAL_PROVER),
        )
        .unwrap();
        let misnamed = env.idl(&mut chain).unwrap_err();

        assert!(
            matches!(&foreign, Error::IdlNotWritable { program, status, .. } if program == HYPER_PROVER && status.contains(&other.to_string())),
            "{foreign:?}"
        );
        assert!(
            matches!(&misnamed, Error::IdlAddress { program, .. } if program == HYPER_PROVER),
            "{misnamed:?}"
        );
        assert!(chain.sent.is_empty(), "{:?}", chain.sent);
    }

    /// Bytecode other than the release's is not the deployer's to describe, even under its
    /// authority.
    #[test]
    fn idl_refuses_a_program_with_other_bytecode() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        env.plan_selection(&chain, Operation::Close, "all").unwrap();
        env.deploy_as(
            &mut chain,
            HYPER_PROVER,
            b"other bytecode",
            Some(env.deployer.pubkey()),
        );

        let error = env.idl(&mut chain).unwrap_err();

        assert!(
            matches!(&error, Error::IdlNotWritable { program, status, .. } if program == HYPER_PROVER && status.contains("hash differs")),
            "{error:?}"
        );
        assert!(chain.sent.is_empty(), "{:?}", chain.sent);
    }

    /// `close-idl` touches only the programs the close plan closes; the closes themselves run
    /// against the metadata program in `tests/idl.rs`.
    #[test]
    fn close_idl_touches_only_the_programs_the_plan_closes() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        env.plan_selection(&chain, Operation::Close, HYPER_PROVER)
            .unwrap();
        chain
            .accounts
            .remove(&idl::canonical_address(&release_address(HYPER_PROVER)));

        let printed = env.close_idl(&mut chain).unwrap();

        assert!(chain.sent.is_empty(), "{:?}", chain.sent);
        assert_eq!(printed, "hyper_prover idl: nothing to close\n");
    }

    #[test]
    fn close_idl_leaves_a_final_program_alone() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        env.plan_selection(&chain, Operation::Close, HYPER_PROVER)
            .unwrap();
        env.deploy_as(&mut chain, HYPER_PROVER, &binary(HYPER_PROVER), None);

        let printed = env.close_idl(&mut chain).unwrap();

        assert!(chain.sent.is_empty(), "{:?}", chain.sent);
        assert!(
            printed.contains("hyper_prover idl left: the program is final"),
            "{printed}"
        );
    }

    #[test]
    fn plan_close_lists_only_the_named_programs_and_their_reserves() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        chain.accounts.insert(
            testing::pda(hyper_prover::state::PDA_PAYER_SEED, HYPER_PROVER),
            Account {
                lamports: 1_234,
                ..Account::default()
            },
        );

        env.plan_selection(&chain, Operation::Close, HYPER_PROVER)
            .unwrap();

        let summary = fs::read_to_string(env.path("summary.md")).unwrap();
        assert_eq!(env.actions(ActionKind::Close), [HYPER_PROVER]);
        assert!(
            summary.contains("- hyper_prover `pda_payer`: 1234 lamports"),
            "{summary}"
        );
    }

    #[test]
    fn plan_close_lists_dependents_first() {
        let env = Env::new();
        let chain = env.tested_chain(&[]);

        env.plan_selection(&chain, Operation::Close, "all").unwrap();

        assert_eq!(
            env.actions(ActionKind::Close),
            [
                AGGREGATOR_PROVER,
                POLYMER_PROVER,
                LOCAL_PROVER,
                LAYERZERO_PROVER,
                HYPER_PROVER,
                CCTP_PROVER
            ]
        );
    }

    #[test]
    fn plan_close_runs_again_after_a_close_that_stopped_part_way() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        env.close(&mut chain, AGGREGATOR_PROVER);

        env.plan_selection(&chain, Operation::Close, "aggregator_prover,hyper_prover")
            .unwrap();

        assert_eq!(env.actions(ActionKind::Close), [HYPER_PROVER]);
    }

    #[test]
    fn a_dependency_outside_the_release_is_refused() {
        let env = Env::new();
        let ids = fs::read_to_string(env.path("ids.json")).unwrap().replacen(
            r#""dependencies":[]"#,
            r#""dependencies":["portal"]"#,
            1,
        );
        fs::write(env.path("ids.json"), ids).unwrap();

        let error = plan(
            &env.chain(),
            &env.plan_args(),
            &env.deployer.pubkey(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(
            matches!(&error, Error::UnknownDependency { dependency, .. } if dependency == "portal"),
            "{error:?}"
        );
    }

    #[test]
    fn a_malformed_keypair_error_never_quotes_the_file() {
        let env = Env::new();
        let secret = "4wBqpZM9xaSheZzJSMawUKKwhdpChKbZ5eu5ky4Vigw8nFNsYzD2CRQ6g6ap5CwmBgdo";
        let path = env.path("deployer.json");
        fs::write(&path, format!("\"{secret}\"")).unwrap();
        let command = Command::Apply(ApplyArgs {
            plan: env.path("plan.json"),
            assets: env.directory.path().into(),
            chain: ChainArgs {
                rpc_url: "unused".into(),
                deployer_keypair: path,
            },
        });

        let error = run(&command, &mut Vec::new()).unwrap_err();

        let printed = format!("{error} {error:?}");
        assert!(!printed.contains(secret), "{printed}");
        assert!(
            printed.contains("not a JSON array of 64 bytes"),
            "{printed}"
        );
    }

    #[test]
    fn a_verification_record_owned_by_another_program_does_not_count() {
        let env = Env::new();
        let mut chain = env.tested_chain(&[]);
        let record = selection::verification_record(
            &env.deployer.pubkey(),
            &release_address(POLYMER_PROVER),
        );
        chain.accounts.get_mut(&record).unwrap().owner = Pubkey::new_unique();

        let error = env
            .plan_selection(&chain, Operation::Finalize, POLYMER_PROVER)
            .unwrap_err();

        assert!(
            matches!(&error, Error::Selection(selection::Error::NotVerified { program }) if program == POLYMER_PROVER),
            "{error:?}"
        );
    }
}
