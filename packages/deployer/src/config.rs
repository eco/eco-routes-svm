use std::fmt;

use anchor_lang::AccountDeserialize;
use eco_svm_std::Bytes32;
use layerzero_prover::state::{Peer, Store};
use solana_sdk::pubkey::Pubkey;
use solana_sdk_ids::{bpf_loader_upgradeable, system_program};

use crate::chain::{self, Chain};
use crate::layerzero_state::{self, Alt, Path};
use crate::plan::{Release, AGGREGATOR_PROVER, HYPER_PROVER, LAYERZERO_PROVER, POLYMER_PROVER};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error(transparent)]
    Uln(#[from] crate::uln::Error),
    #[error("release has no address for {program}")]
    MissingProgram { program: &'static str },
    #[error("{program}: config {address} is owned by {owner}")]
    ForeignOwner {
        program: &'static str,
        address: Pubkey,
        owner: Pubkey,
    },
    #[error("{program}: config does not deserialize: {reason}")]
    Malformed {
        program: &'static str,
        reason: String,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("{program}: config is {actual}, expected {expected}")]
pub struct Mismatch {
    pub program: &'static str,
    pub expected: String,
    pub actual: String,
}

/// The init-once configs of the provers, `None` where the config account does not exist yet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Configs {
    pub hyper_senders: Option<Vec<Bytes32>>,
    pub polymer_emitters: Option<Vec<Bytes32>>,
    pub aggregator_provers: Option<Vec<Pubkey>>,
    pub layerzero_peers: Option<Vec<Peer>>,
    /// The lookup table recorded in the LayerZero `Store`; `None` until `set_alt` ran. Only
    /// observed, never planned, so it is outside [`Self::entries`].
    pub layerzero_alt: Option<Alt>,
    /// One per peer in the `Store`; empty while the `Store` is absent.
    pub layerzero_paths: Vec<Path>,
}

impl Configs {
    /// Fails when a config exists and differs from `expected`; an absent config passes.
    pub fn conflict(&self, expected: &Self) -> Result<(), Mismatch> {
        self.compare(expected, true, |_| true)?;

        self.paths_conflict(expected)
    }

    /// Fails when a config is absent or differs from `expected`. LayerZero paths and the lookup
    /// table are judged apart: see [`Self::paths_conflict`] and `setup::check`.
    pub fn deviation(&self, expected: &Self) -> Result<(), Mismatch> {
        self.compare(expected, false, |_| true)
    }

    /// [`Self::deviation`] for the LayerZero `Store` alone.
    pub fn layerzero_deviation(&self, expected: &Self) -> Result<(), Mismatch> {
        self.compare(expected, false, |program| program == LAYERZERO_PROVER)
    }

    /// Fails when an existing LayerZero path config differs from the one `expected` calls for.
    pub fn paths_conflict(&self, expected: &Self) -> Result<(), Mismatch> {
        self.layerzero_paths.iter().try_for_each(|live| {
            expected
                .layerzero_paths
                .iter()
                .find(|path| path.eid == live.eid)
                .map_or(Ok(()), |path| live.conflict(path))
        })
    }

    pub fn is_absent(&self) -> bool {
        self.entries().iter().all(|(_, values)| values.is_none())
    }

    pub fn entries(&self) -> [(&'static str, Option<Vec<String>>); 4] {
        let Self {
            hyper_senders,
            polymer_emitters,
            aggregator_provers,
            layerzero_peers,
            layerzero_alt: _,
            layerzero_paths: _,
        } = self;

        [
            (HYPER_PROVER, hyper_senders.as_deref().map(bytes32_strings)),
            (
                POLYMER_PROVER,
                polymer_emitters.as_deref().map(bytes32_strings),
            ),
            (
                AGGREGATOR_PROVER,
                aggregator_provers
                    .as_deref()
                    .map(|provers| provers.iter().map(ToString::to_string).collect()),
            ),
            (
                LAYERZERO_PROVER,
                layerzero_peers.as_deref().map(peer_strings),
            ),
        ]
    }

    fn compare(
        &self,
        expected: &Self,
        absent_passes: bool,
        selected: impl Fn(&str) -> bool,
    ) -> Result<(), Mismatch> {
        self.entries()
            .into_iter()
            .zip(expected.entries())
            .filter(|((program, _), _)| selected(program))
            .try_for_each(
                |((program, actual), (_, expected))| match (&actual, &expected) {
                    (None, _) if absent_passes => Ok(()),
                    (actual, expected) if actual == expected => Ok(()),
                    _ => Err(Mismatch {
                        program,
                        expected: describe(&expected),
                        actual: describe(&actual),
                    }),
                },
            )
    }
}

/// One line per config, as the deploy record lists what `apply` read back.
impl fmt::Display for Configs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let alt = self
            .layerzero_alt
            .as_ref()
            .map_or_else(|| "absent".to_owned(), Alt::describe);

        self.entries().iter().try_for_each(|(program, values)| {
            writeln!(formatter, "{program}: {}", describe(values))
        })?;
        writeln!(formatter, "{LAYERZERO_PROVER} lookup table: {alt}")?;
        self.layerzero_paths.iter().try_for_each(|path| {
            writeln!(formatter, "{LAYERZERO_PROVER} path: {}", path.describe())
        })
    }
}

pub fn read(chain: &impl Chain, release: &Release) -> Result<Configs, Error> {
    let address = |program: &'static str| {
        release
            .programs
            .get(program)
            .map(|release_program| release_program.address)
            .ok_or(Error::MissingProgram { program })
    };

    let layerzero = address(LAYERZERO_PROVER)?;
    let store = layerzero_store(chain, &layerzero)?;
    let layerzero_alt = store
        .as_ref()
        .map(|store| layerzero_state::read_alt(chain, store))
        .transpose()?
        .flatten();
    let layerzero_paths = store
        .as_ref()
        .map(|store| layerzero_state::read_paths(chain, &layerzero, store))
        .transpose()?
        .unwrap_or_default();

    Ok(Configs {
        hyper_senders: hyper_senders(chain, &address(HYPER_PROVER)?)?,
        polymer_emitters: polymer_emitters(chain, &address(POLYMER_PROVER)?)?,
        aggregator_provers: aggregator_provers(chain, &address(AGGREGATOR_PROVER)?)?,
        layerzero_peers: store.map(|store| store.peers),
        layerzero_alt,
        layerzero_paths,
    })
}

pub fn layerzero_store(chain: &impl Chain, program: &Pubkey) -> Result<Option<Store>, Error> {
    read_config(
        chain,
        LAYERZERO_PROVER,
        layerzero_prover::state::STORE_SEED,
        program,
    )
}

pub fn hyper_senders(chain: &impl Chain, program: &Pubkey) -> Result<Option<Vec<Bytes32>>, Error> {
    let seed = hyper_prover::state::CONFIG_SEED;
    let config: Option<hyper_prover::state::Config> =
        read_config(chain, HYPER_PROVER, seed, program)?;

    Ok(config.map(|config| config.whitelisted_senders))
}

pub fn polymer_emitters(
    chain: &impl Chain,
    program: &Pubkey,
) -> Result<Option<Vec<Bytes32>>, Error> {
    let seed = polymer_prover::state::CONFIG_SEED;
    let config: Option<polymer_prover::state::Config> =
        read_config(chain, POLYMER_PROVER, seed, program)?;

    Ok(config.map(|config| config.whitelisted_emitters))
}

pub fn aggregator_provers(
    chain: &impl Chain,
    program: &Pubkey,
) -> Result<Option<Vec<Pubkey>>, Error> {
    let seed = aggregator_prover::state::CONFIG_SEED;
    let config: Option<aggregator_prover::state::Config> =
        read_config(chain, AGGREGATOR_PROVER, seed, program)?;

    Ok(config.map(|config| config.provers))
}

/// The config PDA of `program`, derived from its release address rather than the compiled-in ID.
pub fn config_address(seed: &[u8], program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[seed], program).0
}

/// The upgradeable-loader `ProgramData` account of `program`.
pub fn program_data_address(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[program.as_ref()], &bpf_loader_upgradeable::id()).0
}

fn read_config<T: AccountDeserialize>(
    chain: &impl Chain,
    name: &'static str,
    seed: &[u8],
    program: &Pubkey,
) -> Result<Option<T>, Error> {
    let address = config_address(seed, program);
    let Some(account) = chain.account(&address)? else {
        return Ok(None);
    };
    // A pre-funded, never-initialized PDA is still system-owned.
    if account.owner == system_program::id() && account.data.is_empty() {
        return Ok(None);
    }
    if account.owner != *program {
        return Err(Error::ForeignOwner {
            program: name,
            address,
            owner: account.owner,
        });
    }

    T::try_deserialize(&mut account.data.as_slice())
        .map(Some)
        .map_err(|error| Error::Malformed {
            program: name,
            reason: error.to_string(),
        })
}

fn bytes32_strings(values: &[Bytes32]) -> Vec<String> {
    values
        .iter()
        .map(|value| format!("0x{}", hex::encode(**value)))
        .collect()
}

fn peer_strings(peers: &[Peer]) -> Vec<String> {
    peers
        .iter()
        .map(|peer| {
            format!(
                "eid {} chain_id {} address 0x{}",
                peer.eid,
                peer.chain_id,
                hex::encode(*peer.address)
            )
        })
        .collect()
}

fn describe(values: &Option<Vec<String>>) -> String {
    match values {
        Some(values) => format!("[{}]", values.join(", ")),
        None => "absent".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{full_setup, plan};

    #[test]
    fn displays_one_line_per_config() {
        let plan = plan(5_000_000);
        let setup = full_setup(&plan);

        let configs = read(&setup.chain, &plan.release).unwrap();

        goldie::assert!(configs.to_string());
    }
}
