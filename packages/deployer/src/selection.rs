use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;

use crate::chain::{self, Chain};
use crate::classify::{self, classify, ProgramState, Status};
use crate::config::{self, Configs};
use crate::layerzero_state::{Alt, Path};
use crate::plan::{
    self, Action, Cluster, PlanHash, PlannedProgram, PlannedProgramFile, PlannedStatus, Release,
    HYPER_PROVER, LAYERZERO_PROVER, PROGRAMS_WITH_INIT,
};
use crate::setup;

/// OtterSec's verification program: `solana-verify verify-from-repo` records each verified build
/// there, at a PDA of the uploader and the program.
const VERIFY_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC");
const VERIFY_SEED: &[u8] = b"otter_verify";
const ALL: &str = "all";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error(transparent)]
    Classify(#[from] classify::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error(transparent)]
    Plan(#[from] plan::Error),
    #[error(transparent)]
    LayerZeroIncomplete(#[from] setup::Incomplete),
    #[error("no programs selected: pass `all` or a comma-separated list of program names")]
    EmptySelection,
    #[error("{program} is not a program of this release")]
    UnknownProgram { program: String },
    #[error("no on-chain state for {program}")]
    MissingState { program: String },
    #[error("{program} is not ours: {reason}")]
    Foreign { program: String, reason: String },
    #[error("{program} is not deployed; run the deploy workflow first")]
    NotDeployed { program: String },
    #[error("{program} is already final and cannot be closed")]
    AlreadyFinal { program: String },
    #[error("{program} has no config; a final program could never be initialized")]
    NotInitialized { program: String },
    #[error("{program} has no verification record from the deployer; the deploy workflow's verify must succeed first")]
    NotVerified { program: String },
    #[error("{program} depends on {dependency}, which is neither final nor finalized in this run")]
    DependencyNotFinal { program: String, dependency: String },
    #[error("{program} cannot be closed: the final {dependent} depends on it")]
    FinalDependent { program: String, dependent: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Finalize,
    Close,
}

/// The `programs` input: every program of the release, or the named ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Programs {
    All,
    Named(BTreeSet<String>),
}

/// What the chain holds for a release, as a finalize or close plan reads it.
#[derive(Debug, Clone)]
pub struct Observed {
    pub genesis_hash: Hash,
    pub states: BTreeMap<String, ProgramState>,
    pub live_configs: Configs,
    /// Programs with a verification record uploaded by the deployer.
    pub verified: BTreeSet<String>,
    /// Lamports in the hyper and LayerZero `pda_payer` reserves; closing a program strands its
    /// reserve. Not hashed: inbound deliveries move them.
    pub reserves: BTreeMap<String, u64>,
}

/// A reviewed set of upgradeable programs to finalize or close.
#[derive(Debug, Clone)]
pub struct Selection {
    pub operation: Operation,
    pub release: Release,
    pub genesis_hash: Hash,
    pub deployer: Pubkey,
    pub programs: BTreeMap<String, PlannedProgram>,
    pub live_configs: Configs,
    pub verified: BTreeSet<String>,
    pub reserves: BTreeMap<String, u64>,
}

/// What `deployer plan-finalize` / `plan-close` writes; `actions` reads `release` and `programs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionFile {
    pub operation: Operation,
    pub release: Release,
    pub selected: String,
    pub genesis_hash: String,
    pub hash: String,
    pub programs: BTreeMap<String, PlannedProgramFile>,
}

pub fn observe(
    chain: &impl Chain,
    release: &Release,
    binaries: &BTreeMap<String, Vec<u8>>,
    deployer: &Pubkey,
) -> Result<Observed, Error> {
    let states = release
        .programs
        .iter()
        .map(|(name, program)| {
            let state = classify(chain, &program.address, &binaries[name], deployer)?;

            Ok((name.clone(), state))
        })
        .collect::<Result<_, Error>>()?;
    let records: Vec<(String, bool)> = release
        .programs
        .iter()
        .map(|(name, program)| {
            let record = verification_record(deployer, &program.address);
            let recorded = chain
                .account(&record)?
                .is_some_and(|account| account.owner == VERIFY_PROGRAM_ID);

            Ok((name.clone(), recorded))
        })
        .collect::<Result<_, Error>>()?;
    let verified = records
        .into_iter()
        .filter_map(|(name, recorded)| recorded.then_some(name))
        .collect();
    let reserves = [
        (HYPER_PROVER, hyper_prover::state::PDA_PAYER_SEED),
        (LAYERZERO_PROVER, layerzero_prover::state::PDA_PAYER_SEED),
    ]
    .into_iter()
    .filter_map(|(program, seed)| {
        release
            .programs
            .get(program)
            .map(|released| (program, seed, released))
    })
    .map(|(program, seed, released)| {
        let reserve = Pubkey::find_program_address(&[seed], &released.address).0;
        let lamports = chain
            .account(&reserve)?
            .map_or(0, |account| account.lamports);

        Ok((program.to_owned(), lamports))
    })
    .collect::<Result<_, Error>>()?;

    Ok(Observed {
        genesis_hash: chain.genesis_hash()?,
        states,
        live_configs: config::read(chain, release)?,
        verified,
        reserves,
    })
}

/// `solana-verify`'s build-parameters PDA for `program`, as uploaded by `signer`.
pub fn verification_record(signer: &Pubkey, program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[VERIFY_SEED, signer.as_ref(), program.as_ref()],
        &VERIFY_PROGRAM_ID,
    )
    .0
}

impl Selection {
    pub fn build(
        operation: Operation,
        selected: &Programs,
        release: Release,
        deployer: Pubkey,
        observed: Observed,
    ) -> Result<Self, Error> {
        let Observed {
            genesis_hash,
            mut states,
            live_configs,
            verified,
            reserves,
        } = observed;
        release.cluster.require_genesis(genesis_hash)?;
        reject_unknown(selected, &release)?;
        let states = release
            .programs
            .keys()
            .map(|name| {
                let state = states.remove(name).ok_or_else(|| Error::MissingState {
                    program: name.clone(),
                })?;
                if let Status::Foreign { reason } = &state.status {
                    return Err(Error::Foreign {
                        program: name.clone(),
                        reason: reason.clone(),
                    });
                }

                Ok((name.clone(), state))
            })
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        let targets = targets(operation, selected, &states)?;
        let checks = Checks {
            release: &release,
            states: &states,
            targets: &targets,
            live_configs: &live_configs,
            verified: &verified,
        };
        targets.iter().try_for_each(|program| match operation {
            Operation::Finalize => checks.finalizable(program),
            Operation::Close => checks.closable(program),
        })?;
        let action = match operation {
            Operation::Finalize => Action::Finalize,
            Operation::Close => Action::Close,
        };
        let programs = states
            .into_iter()
            .map(|(name, state)| {
                let actions = match targets.contains(&name) {
                    true => vec![action],
                    false => vec![],
                };

                (name, PlannedProgram { state, actions })
            })
            .collect();

        Ok(Self {
            operation,
            release,
            genesis_hash,
            deployer,
            programs,
            live_configs,
            verified,
            reserves,
        })
    }

    pub fn hash(&self) -> PlanHash {
        PlanHash::of(&self.canonical_json())
    }

    pub fn file(&self, selected: String) -> SelectionFile {
        SelectionFile {
            operation: self.operation,
            release: self.release.clone(),
            selected,
            genesis_hash: self.genesis_hash.to_string(),
            hash: self.hash().to_string(),
            programs: self
                .programs
                .iter()
                .map(|(name, program)| (name.clone(), program.into()))
                .collect(),
        }
    }

    pub fn summary(&self) -> String {
        let Self {
            operation,
            release,
            programs,
            live_configs,
            ..
        } = self;
        let table = programs.iter().fold(
            "| Program | Address | Status | Action |\n|---|---|---|---|\n".to_owned(),
            |table, (name, program)| {
                let action = match program.actions.is_empty() {
                    true => "none".to_owned(),
                    false => operation.to_string(),
                };
                let status: PlannedStatus = (&program.state.status).into();

                table
                    + &format!(
                        "| {name} | `{}` | {status} | {action} |\n",
                        program.state.address
                    )
            },
        );
        let details = match operation {
            Operation::Finalize => format!("### On-chain configs\n\n```\n{live_configs}```\n"),
            Operation::Close => self.close_warning(),
        };

        format!(
            "## {} plan v{} on {}\n\n{table}\n{details}\nPlan hash: {}\n",
            operation.title(),
            release.version,
            release.cluster,
            self.hash(),
        )
    }

    fn close_warning(&self) -> String {
        let stranded = self
            .reserves
            .iter()
            .filter(|(program, _)| self.programs[*program].actions.contains(&Action::Close))
            .map(|(program, lamports)| format!("- {program} `pda_payer`: {lamports} lamports\n"))
            .collect::<String>();

        format!(
            "Closing returns each program's rent to the deployer and burns its address: it can \
             never be deployed again. Lamports held by the program's own accounts stay locked \
             for good, including LayerZero's endpoint and ULN account rent and these reserves:\n\n\
             {stranded}"
        )
    }

    fn canonical_json(&self) -> String {
        let Self {
            operation,
            release,
            genesis_hash,
            deployer,
            programs,
            live_configs,
            verified,
            ..
        } = self;
        let canonical = CanonicalSelection {
            operation: *operation,
            version: &release.version,
            cluster: release.cluster,
            genesis_hash: genesis_hash.to_string(),
            deployer: deployer.to_string(),
            programs: programs
                .iter()
                .map(|(name, program)| {
                    let canonical = CanonicalProgram {
                        address: program.state.address.to_string(),
                        so_sha256: hex::encode(release.programs[name].so_sha256),
                        dependencies: release.programs[name].dependencies.clone(),
                        planned: program.into(),
                    };

                    (name.as_str(), canonical)
                })
                .collect(),
            live_configs: live_configs.entries().into_iter().collect(),
            live_layerzero_alt: live_configs.layerzero_alt.as_ref().map(Alt::describe),
            live_layerzero_paths: live_configs
                .layerzero_paths
                .iter()
                .map(Path::describe)
                .collect(),
            verified,
        };

        serde_json::to_string(&canonical).expect("canonical selection must serialize")
    }
}

struct Checks<'a> {
    release: &'a Release,
    states: &'a BTreeMap<String, ProgramState>,
    targets: &'a BTreeSet<String>,
    live_configs: &'a Configs,
    verified: &'a BTreeSet<String>,
}

impl Checks<'_> {
    /// A final program can no longer be initialized, verified by its authority, or protected
    /// from a dependency that is later closed.
    fn finalizable(&self, program: &String) -> Result<(), Error> {
        let configured = self
            .live_configs
            .entries()
            .into_iter()
            .any(|(name, values)| name == program && values.is_some());
        if PROGRAMS_WITH_INIT.contains(&program.as_str()) && !configured {
            return Err(Error::NotInitialized {
                program: program.clone(),
            });
        }
        if program == LAYERZERO_PROVER {
            setup::check(self.live_configs)?;
        }
        if !self.verified.contains(program) {
            return Err(Error::NotVerified {
                program: program.clone(),
            });
        }

        self.release.programs[program]
            .dependencies
            .iter()
            .find(|dependency| {
                self.states[*dependency].status != Status::Live
                    && !self.targets.contains(*dependency)
            })
            .map_or(Ok(()), |dependency| {
                Err(Error::DependencyNotFinal {
                    program: program.clone(),
                    dependency: dependency.clone(),
                })
            })
    }

    /// Finalize refuses a program before its dependencies, so no final program should depend on
    /// one that is still upgradeable; this keeps that true even for programs finalized by hand.
    fn closable(&self, program: &String) -> Result<(), Error> {
        self.release
            .programs
            .iter()
            .find(|(name, released)| {
                self.states[*name].status == Status::Live && released.dependencies.contains(program)
            })
            .map_or(Ok(()), |(dependent, _)| {
                Err(Error::FinalDependent {
                    program: program.clone(),
                    dependent: dependent.clone(),
                })
            })
    }
}

fn reject_unknown(selected: &Programs, release: &Release) -> Result<(), Error> {
    let Programs::Named(named) = selected else {
        return Ok(());
    };

    named
        .iter()
        .find(|program| !release.programs.contains_key(*program))
        .map_or(Ok(()), |program| {
            Err(Error::UnknownProgram {
                program: program.clone(),
            })
        })
}

/// The selected programs still upgradeable. Finalize refuses an undeployed one; close refuses a
/// final one it was asked for by name.
fn targets(
    operation: Operation,
    selected: &Programs,
    states: &BTreeMap<String, ProgramState>,
) -> Result<BTreeSet<String>, Error> {
    let named = |name: &String| match selected {
        Programs::All => true,
        Programs::Named(named) => named.contains(name),
    };

    states
        .iter()
        .filter(|(name, _)| named(name))
        .filter_map(|(name, state)| match (operation, &state.status, selected) {
            (_, Status::Partial, _) => Some(Ok(name.clone())),
            (Operation::Finalize, Status::New, _) => Some(Err(Error::NotDeployed {
                program: name.clone(),
            })),
            (Operation::Close, Status::Live, Programs::Named(_)) => {
                Some(Err(Error::AlreadyFinal {
                    program: name.clone(),
                }))
            }
            _ => None,
        })
        .collect()
}

impl Operation {
    fn title(self) -> &'static str {
        match self {
            Self::Finalize => "Finalize",
            Self::Close => "Close",
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Finalize => "finalize",
            Self::Close => "close",
        })
    }
}

impl FromStr for Programs {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim() == ALL {
            return Ok(Self::All);
        }
        let named: BTreeSet<String> = value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(Into::into)
            .collect();

        match named.is_empty() {
            true => Err(Error::EmptySelection),
            false => Ok(Self::Named(named)),
        }
    }
}

#[derive(Serialize)]
struct CanonicalSelection<'a> {
    operation: Operation,
    version: &'a str,
    cluster: Cluster,
    genesis_hash: String,
    deployer: String,
    programs: BTreeMap<&'a str, CanonicalProgram>,
    live_configs: BTreeMap<&'static str, Option<Vec<String>>>,
    live_layerzero_alt: Option<String>,
    live_layerzero_paths: Vec<String>,
    verified: &'a BTreeSet<String>,
}

#[derive(Serialize)]
struct CanonicalProgram {
    address: String,
    so_sha256: String,
    dependencies: Vec<String>,
    #[serde(flatten)]
    planned: PlannedProgramFile,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{AGGREGATOR_PROVER, POLYMER_PROVER};
    use crate::testing::{self, address, full_setup};

    fn deployer() -> Pubkey {
        address(1)
    }

    /// Every fixture program in `status`, initialized, its LayerZero setup complete and every
    /// program verified.
    fn observed(status: Status) -> (Release, Observed) {
        let plan = testing::plan(5_000_000);
        let setup = full_setup(&plan);
        let live_configs = config::read(&setup.chain, &plan.release).unwrap();
        let states = plan
            .release
            .programs
            .iter()
            .map(|(name, program)| {
                let state = ProgramState {
                    address: program.address,
                    status: status.clone(),
                    authority: Some(deployer()),
                    data_hash: Some([1; 32]),
                };

                (name.clone(), state)
            })
            .collect();
        let verified = plan.release.programs.keys().cloned().collect();
        let observed = Observed {
            genesis_hash: Cluster::Devnet.genesis_hash(),
            states,
            live_configs,
            verified,
            reserves: BTreeMap::from([(HYPER_PROVER.into(), 7), (LAYERZERO_PROVER.into(), 9)]),
        };

        (plan.release, observed)
    }

    fn build(
        operation: Operation,
        selected: &str,
        change: impl FnOnce(&mut Observed),
    ) -> Result<Selection, Error> {
        let (release, mut observed) = observed(Status::Partial);
        change(&mut observed);

        Selection::build(operation, &selected.parse()?, release, deployer(), observed)
    }

    fn targeted(selection: &Selection) -> Vec<&str> {
        selection
            .programs
            .iter()
            .filter(|(_, program)| !program.actions.is_empty())
            .map(|(name, _)| name.as_str())
            .collect()
    }

    fn set_status(observed: &mut Observed, program: &str, status: Status) {
        observed.states.get_mut(program).unwrap().status = status;
    }

    #[test]
    fn programs_parse_all_or_a_list() {
        assert_eq!("all".parse::<Programs>().unwrap(), Programs::All);
        assert_eq!(
            " hyper_prover, polymer_prover "
                .parse::<Programs>()
                .unwrap(),
            Programs::Named([HYPER_PROVER.into(), POLYMER_PROVER.into()].into())
        );
        ["", " , "].iter().for_each(|value| {
            assert!(matches!(
                value.parse::<Programs>(),
                Err(Error::EmptySelection)
            ));
        });
    }

    #[test]
    fn finalize_all_targets_every_upgradeable_program() {
        let selection = build(Operation::Finalize, "all", |_| {}).unwrap();

        assert_eq!(targeted(&selection).len(), selection.programs.len());
        assert!(selection
            .programs
            .values()
            .all(|program| program.actions == [Action::Finalize]));
    }

    #[test]
    fn finalize_skips_final_programs_and_refuses_undeployed_ones() {
        let live = build(Operation::Finalize, "all", |observed| {
            set_status(observed, HYPER_PROVER, Status::Live)
        })
        .unwrap();
        let new = build(Operation::Finalize, "all", |observed| {
            set_status(observed, HYPER_PROVER, Status::New)
        });

        assert!(!targeted(&live).contains(&HYPER_PROVER));
        assert!(matches!(new, Err(Error::NotDeployed { program }) if program == HYPER_PROVER));
    }

    #[test]
    fn finalize_refuses_a_program_before_its_dependencies() {
        let alone = build(Operation::Finalize, AGGREGATOR_PROVER, |_| {});
        let with_members = build(
            Operation::Finalize,
            "aggregator_prover,hyper_prover,polymer_prover,layerzero_prover",
            |_| {},
        );
        let members_final = build(Operation::Finalize, AGGREGATOR_PROVER, |observed| {
            [HYPER_PROVER, POLYMER_PROVER, LAYERZERO_PROVER]
                .into_iter()
                .for_each(|member| set_status(observed, member, Status::Live));
        });

        assert!(matches!(
            alone,
            Err(Error::DependencyNotFinal { program, dependency })
                if program == AGGREGATOR_PROVER && dependency == HYPER_PROVER
        ));
        assert!(with_members.is_ok());
        assert_eq!(targeted(&members_final.unwrap()), [AGGREGATOR_PROVER]);
    }

    #[test]
    fn finalize_refuses_an_unverified_or_uninitialized_program() {
        let unverified = build(Operation::Finalize, HYPER_PROVER, |observed| {
            observed.verified.remove(HYPER_PROVER);
        });
        let uninitialized = build(Operation::Finalize, HYPER_PROVER, |observed| {
            observed.live_configs.hyper_senders = None;
        });

        assert!(
            matches!(unverified, Err(Error::NotVerified { program }) if program == HYPER_PROVER)
        );
        assert!(matches!(
            uninitialized,
            Err(Error::NotInitialized { program }) if program == HYPER_PROVER
        ));
    }

    #[test]
    fn finalize_refuses_layerzero_without_its_whole_setup() {
        let incomplete: [fn(&mut Configs); 4] = [
            |configs| configs.layerzero_alt = None,
            |configs| configs.layerzero_paths[0].nonce = false,
            |configs| configs.layerzero_paths[0].send = None,
            |configs| configs.layerzero_paths[0].receive = None,
        ];

        incomplete.into_iter().for_each(|change| {
            let result = build(Operation::Finalize, LAYERZERO_PROVER, |observed| {
                change(&mut observed.live_configs)
            });

            assert!(
                matches!(result, Err(Error::LayerZeroIncomplete(_))),
                "{result:?}"
            );
        });
    }

    #[test]
    fn close_targets_upgradeable_programs_and_refuses_a_named_final_one() {
        let all = build(Operation::Close, "all", |observed| {
            set_status(observed, HYPER_PROVER, Status::Live);
            set_status(observed, POLYMER_PROVER, Status::New);
        })
        .unwrap();
        let named_final = build(Operation::Close, HYPER_PROVER, |observed| {
            set_status(observed, HYPER_PROVER, Status::Live)
        });

        assert!(!targeted(&all).contains(&HYPER_PROVER));
        assert!(!targeted(&all).contains(&POLYMER_PROVER));
        assert!(all
            .programs
            .values()
            .filter(|program| !program.actions.is_empty())
            .all(|program| program.actions == [Action::Close]));
        assert!(matches!(
            named_final,
            Err(Error::AlreadyFinal { program }) if program == HYPER_PROVER
        ));
    }

    #[test]
    fn close_refuses_a_dependency_of_a_final_program() {
        let result = build(Operation::Close, HYPER_PROVER, |observed| {
            set_status(observed, AGGREGATOR_PROVER, Status::Live)
        });

        assert!(matches!(
            result,
            Err(Error::FinalDependent { program, dependent })
                if program == HYPER_PROVER && dependent == AGGREGATOR_PROVER
        ));
    }

    #[test]
    fn unknown_foreign_and_other_cluster_are_refused() {
        let unknown = build(Operation::Finalize, "portal", |_| {});
        let foreign = build(Operation::Close, "all", |observed| {
            set_status(
                observed,
                HYPER_PROVER,
                Status::Foreign {
                    reason: "hash differs".into(),
                },
            )
        });
        let mainnet = build(Operation::Close, "all", |observed| {
            observed.genesis_hash = Cluster::Mainnet.genesis_hash()
        });

        assert!(matches!(unknown, Err(Error::UnknownProgram { program }) if program == "portal"));
        assert!(matches!(foreign, Err(Error::Foreign { program, .. }) if program == HYPER_PROVER));
        assert!(matches!(
            mainnet,
            Err(Error::Plan(plan::Error::ClusterMismatch { .. }))
        ));
    }

    #[test]
    fn hash_covers_the_operation_the_selection_and_the_verified_set() {
        let hash = |operation, selected, change: fn(&mut Observed)| {
            build(operation, selected, change).unwrap().hash()
        };
        let base = hash(Operation::Finalize, "all", |_| {});

        assert_eq!(base, hash(Operation::Finalize, "all", |_| {}));
        assert_ne!(base, hash(Operation::Close, "all", |_| {}));
        assert_ne!(base, hash(Operation::Finalize, HYPER_PROVER, |_| {}));
        assert_ne!(
            hash(Operation::Close, HYPER_PROVER, |_| {}),
            hash(Operation::Close, HYPER_PROVER, |observed| {
                observed.verified.remove(HYPER_PROVER);
            })
        );
        assert_eq!(
            hash(Operation::Close, HYPER_PROVER, |_| {}),
            hash(Operation::Close, HYPER_PROVER, |observed| {
                observed.reserves.clear();
            })
        );
    }

    #[test]
    fn finalize_summary_lists_programs_and_live_configs() {
        goldie::assert!(build(Operation::Finalize, "all", |_| {}).unwrap().summary());
    }

    #[test]
    fn close_summary_names_the_reserves_left_behind() {
        goldie::assert!(
            build(Operation::Close, "hyper_prover,layerzero_prover", |_| {})
                .unwrap()
                .summary()
        );
    }
}
