use crate::chain::{self, Chain};
use crate::config::{self, Configs};
use crate::plan::Plan;
use crate::setup;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error(transparent)]
    Incomplete(#[from] setup::Incomplete),
    #[error("{program}: on-chain config is {actual}, expected {expected}")]
    Mismatch {
        program: &'static str,
        expected: String,
        actual: String,
    },
}

/// Every init-once config must be on chain and equal to the plan's inputs, and the LayerZero
/// paths and lookup table must be complete. Returns what it read.
pub fn readback(chain: &impl Chain, plan: &Plan) -> Result<Configs, Error> {
    let Plan {
        release,
        expected_configs,
        ..
    } = plan;
    let live: Configs = config::read(chain, release)?;
    live.deviation(expected_configs)?;
    setup::check(&live)?;
    live.paths_conflict(expected_configs)?;

    Ok(live)
}

impl From<config::Mismatch> for Error {
    fn from(mismatch: config::Mismatch) -> Self {
        let config::Mismatch {
            program,
            expected,
            actual,
        } = mismatch;

        Self::Mismatch {
            program,
            expected,
            actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::account::Account;
    use solana_sdk::pubkey::Pubkey;

    use super::*;
    use crate::plan::LAYERZERO_PROVER;
    use crate::testing::{full_setup, plan, uln_account_data};

    #[test]
    fn complete_setup_reads_back() {
        let plan = plan(5_000_000);
        let setup = full_setup(&plan);

        assert!(readback(&setup.chain, &plan).is_ok());
    }

    #[test]
    fn each_missing_or_wrong_part_is_named() {
        type Break = fn(&mut crate::testing::FullSetup, &Plan);
        type Check = fn(&Error) -> bool;
        let plan = plan(5_000_000);
        let cases: [(&str, Break, Check); 9] = [
            (
                "store absent",
                |setup, _| {
                    setup.chain.accounts.remove(&setup.store);
                },
                |error| matches!(error, Error::Mismatch { program, .. } if *program == LAYERZERO_PROVER),
            ),
            (
                "nonce missing",
                |setup, _| {
                    setup.chain.accounts.remove(&setup.nonce);
                },
                |error| {
                    matches!(
                        error,
                        Error::Incomplete(setup::Incomplete::PathMissing { .. })
                    )
                },
            ),
            (
                "send config missing",
                |setup, _| {
                    setup.chain.accounts.remove(&setup.send_config);
                },
                |error| {
                    matches!(
                        error,
                        Error::Incomplete(setup::Incomplete::PathMissing { .. })
                    )
                },
            ),
            (
                "receive config missing",
                |setup, _| {
                    setup.chain.accounts.remove(&setup.receive_config);
                },
                |error| {
                    matches!(
                        error,
                        Error::Incomplete(setup::Incomplete::PathMissing { .. })
                    )
                },
            ),
            (
                "send config with another executor",
                |setup, plan| {
                    let uln = layerzero_prover::layerzero::ULN_ID;
                    let send = plan.expected_configs.layerzero_paths[0]
                        .send
                        .clone()
                        .unwrap();
                    let mut executor = send.executor;
                    executor.max_message_size += 1;
                    setup.chain.accounts.insert(
                        setup.send_config,
                        Account {
                            owner: uln,
                            data: uln_account_data("SendConfig", &(255u8, send.uln, executor)),
                            ..Account::default()
                        },
                    );
                },
                |error| matches!(error, Error::Mismatch { program, .. } if *program == LAYERZERO_PROVER),
            ),
            (
                "alt unfrozen",
                |setup, _| {
                    setup.chain.accounts.get_mut(&setup.alt).unwrap().data[21] = 1;
                },
                |error| {
                    matches!(
                        error,
                        Error::Incomplete(setup::Incomplete::AltUnfrozen { .. })
                    )
                },
            ),
            (
                "alt deactivated",
                |setup, _| {
                    setup.chain.accounts.get_mut(&setup.alt).unwrap().data[4..12]
                        .copy_from_slice(&5u64.to_le_bytes());
                },
                |error| {
                    matches!(
                        error,
                        Error::Incomplete(setup::Incomplete::AltDeactivated { .. })
                    )
                },
            ),
            (
                "alt incomplete",
                |setup, _| {
                    let data = &mut setup.chain.accounts.get_mut(&setup.alt).unwrap().data;
                    data.truncate(data.len() - 32);
                },
                |error| {
                    matches!(
                        error,
                        Error::Incomplete(setup::Incomplete::AltIncomplete { .. })
                    )
                },
            ),
            (
                "alt not recorded",
                |setup, _| {
                    let store = setup.chain.accounts.get_mut(&setup.store).unwrap();
                    let alt_offset = store.data.len() - 32;
                    store.data[alt_offset..].copy_from_slice(Pubkey::default().as_ref());
                },
                |error| matches!(error, Error::Incomplete(setup::Incomplete::AltNotSet)),
            ),
        ];

        cases.into_iter().for_each(|(name, break_it, expected)| {
            let mut setup = full_setup(&plan);
            break_it(&mut setup, &plan);

            let error = readback(&setup.chain, &plan).unwrap_err();

            assert!(expected(&error), "{name}: {error:?}");
        });
    }
}
