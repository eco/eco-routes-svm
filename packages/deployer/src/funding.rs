use std::collections::BTreeMap;
use std::fmt;

use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;

use crate::chain::{self, Chain};
use crate::plan::{Action, Plan, HYPER_PROVER, LAYERZERO_PROVER};

/// Transaction fees, the configs', `Store`'s and lookup table's rent, and `solana-verify`'s
/// verification PDAs. Each is far below this for any input set that fits a transaction.
const ALLOWANCE_LAMPORTS: u64 = 200_000_000;
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error(
        "deployer {deployer} holds {balance}, the run needs {required}; fund it and plan again"
    )]
    InsufficientBalance {
        deployer: Pubkey,
        balance: Sol,
        required: Sol,
    },
}

/// What a run takes from the deployer, against what it holds. Not part of the plan hash: every
/// fee moves the balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Funding {
    deployer: Pubkey,
    balance: Sol,
    /// Program and program-data rent of every program the run deploys; locked for good once
    /// the program is final.
    program_rent: Sol,
    /// `solana program deploy` writes one buffer at a time and refunds it into the program data.
    largest_buffer: Sol,
    reserve_top_ups: Sol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sol(u64);

impl Funding {
    pub fn estimate(
        chain: &impl Chain,
        plan: &Plan,
        binaries: &BTreeMap<String, Vec<u8>>,
        deployer: &Pubkey,
    ) -> Result<Self, Error> {
        let rent = Rent::default();
        let deployed_lengths: Vec<usize> = plan
            .programs
            .iter()
            .filter(|(_, program)| program.actions.contains(&Action::Deploy))
            .map(|(name, _)| binaries[name].len())
            .collect();
        let program_rent = deployed_lengths
            .iter()
            .map(|length| {
                rent.minimum_balance(UpgradeableLoaderState::size_of_programdata(*length))
                    + rent.minimum_balance(UpgradeableLoaderState::size_of_program())
            })
            .sum();
        let largest_buffer = deployed_lengths
            .iter()
            .map(|length| rent.minimum_balance(UpgradeableLoaderState::size_of_buffer(*length)))
            .max()
            .unwrap_or(0);
        let reserves = [
            (
                HYPER_PROVER,
                hyper_prover::state::PDA_PAYER_SEED,
                plan.inputs.hyper_reserve_lamports,
            ),
            (
                LAYERZERO_PROVER,
                layerzero_prover::state::PDA_PAYER_SEED,
                plan.inputs.layerzero_reserve_lamports,
            ),
        ];
        let reserve_top_ups = reserves
            .into_iter()
            .map(|(program, seed, requested_lamports)| {
                let address = plan.release.programs[program].address;
                let reserve = Pubkey::find_program_address(&[seed], &address).0;

                reserve_top_up(chain, &reserve, requested_lamports)
            })
            .sum::<Result<u64, _>>()?;

        Ok(Self {
            deployer: *deployer,
            balance: Sol(lamports(chain, deployer)?),
            program_rent: Sol(program_rent),
            largest_buffer: Sol(largest_buffer),
            reserve_top_ups: Sol(reserve_top_ups),
        })
    }

    pub fn require(&self) -> Result<(), Error> {
        match self.balance >= self.required() {
            true => Ok(()),
            false => Err(Error::InsufficientBalance {
                deployer: self.deployer,
                balance: self.balance,
                required: self.required(),
            }),
        }
    }

    fn required(&self) -> Sol {
        Sol(self.program_rent.0
            + self.largest_buffer.0
            + self.reserve_top_ups.0
            + ALLOWANCE_LAMPORTS)
    }
}

/// What `apply` transfers to reach `requested_lamports` in `reserve`; it never takes any back.
pub fn reserve_top_up(
    chain: &impl Chain,
    reserve: &Pubkey,
    requested_lamports: u64,
) -> Result<u64, chain::Error> {
    Ok(target_lamports(requested_lamports).saturating_sub(lamports(chain, reserve)?))
}

/// A reserve below the rent-exempt minimum would be a rent-paying account, which the runtime
/// rejects. `Rent::default()` rather than the sysvar: `Chain` exposes accounts only, and the
/// minimum for a zero-data account is the same on every cluster we deploy to.
fn target_lamports(requested_lamports: u64) -> u64 {
    match requested_lamports {
        0 => 0,
        requested => requested.max(Rent::default().minimum_balance(0)),
    }
}

fn lamports(chain: &impl Chain, address: &Pubkey) -> Result<u64, chain::Error> {
    Ok(chain
        .account(address)?
        .map_or(0, |account| account.lamports))
}

impl fmt::Display for Funding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            deployer,
            balance,
            program_rent,
            largest_buffer,
            reserve_top_ups,
        } = self;
        let required = self.required();
        let verdict = match balance >= &required {
            true => "Funded.".to_owned(),
            false => format!(
                "**Short by {}**: fund `{deployer}`, then plan again.",
                Sol(required.0 - balance.0)
            ),
        };

        write!(
            formatter,
            "### Deployer funding\n\n\
             | | Amount |\n|---|---|\n\
             | Program rent (locked once final) | {program_rent} |\n\
             | Largest buffer (refunded) | {largest_buffer} |\n\
             | Reserve top-ups | {reserve_top_ups} |\n\
             | Fees and account rent allowance | {} |\n\
             | **Required** | {required} |\n\
             | Balance of `{deployer}` | {balance} |\n\n\
             {verdict}\n",
            Sol(ALLOWANCE_LAMPORTS),
        )
    }
}

impl fmt::Display for Sol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}.{:09} SOL",
            self.0 / LAMPORTS_PER_SOL,
            self.0 % LAMPORTS_PER_SOL
        )
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::account::Account;

    use super::*;
    use crate::testing::{self, pda, release_address, RecordingChain};

    const DEPLOYER: Pubkey = Pubkey::new_from_array([7; 32]);
    const RESERVE_LAMPORTS: u64 = 5_000_000;

    fn funded(lamports: u64) -> RecordingChain {
        let mut chain = RecordingChain::default();
        chain.accounts.insert(
            DEPLOYER,
            Account {
                lamports,
                ..Account::default()
            },
        );

        chain
    }

    fn binaries(plan: &Plan) -> BTreeMap<String, Vec<u8>> {
        plan.programs
            .keys()
            .map(|name| (name.clone(), vec![0; 1_000]))
            .collect()
    }

    fn rent(bytes: usize) -> u64 {
        Rent::default().minimum_balance(bytes)
    }

    #[test]
    fn partial_programs_cost_only_reserves_and_the_allowance() {
        let plan = testing::plan(RESERVE_LAMPORTS);

        let funding = Funding::estimate(&funded(0), &plan, &binaries(&plan), &DEPLOYER).unwrap();

        assert_eq!(
            funding.required(),
            Sol(2 * RESERVE_LAMPORTS + ALLOWANCE_LAMPORTS)
        );
    }

    #[test]
    fn deployed_programs_add_their_rent_and_the_largest_buffer() {
        let mut plan = testing::plan(0);
        plan.programs
            .values_mut()
            .for_each(|program| program.actions = vec![Action::Deploy]);
        let mut binaries = binaries(&plan);
        binaries.insert(LAYERZERO_PROVER.into(), vec![0; 3_000]);

        let funding = Funding::estimate(&funded(0), &plan, &binaries, &DEPLOYER).unwrap();

        assert_eq!(
            funding.required(),
            Sol(4 * (rent(45 + 1_000) + rent(36))
                + rent(45 + 3_000)
                + rent(36)
                + rent(37 + 3_000)
                + ALLOWANCE_LAMPORTS)
        );
    }

    #[test]
    fn reserves_count_only_what_they_lack() {
        let plan = testing::plan(RESERVE_LAMPORTS);
        let mut chain = funded(0);
        chain.accounts.insert(
            pda(hyper_prover::state::PDA_PAYER_SEED, HYPER_PROVER),
            Account {
                lamports: RESERVE_LAMPORTS - 1,
                ..Account::default()
            },
        );
        chain.accounts.insert(
            pda(layerzero_prover::state::PDA_PAYER_SEED, LAYERZERO_PROVER),
            Account {
                lamports: RESERVE_LAMPORTS + 1,
                ..Account::default()
            },
        );

        let funding = Funding::estimate(&chain, &plan, &binaries(&plan), &DEPLOYER).unwrap();

        assert_eq!(funding.required(), Sol(1 + ALLOWANCE_LAMPORTS));
    }

    #[test]
    fn require_rejects_a_balance_below_the_estimate() {
        let plan = testing::plan(0);
        let estimate = |lamports| {
            Funding::estimate(&funded(lamports), &plan, &binaries(&plan), &DEPLOYER).unwrap()
        };

        assert!(estimate(ALLOWANCE_LAMPORTS).require().is_ok());
        assert!(estimate(ALLOWANCE_LAMPORTS - 1)
            .require()
            .is_err_and(|error| matches!(
                error,
                Error::InsufficientBalance { deployer, balance, required }
                    if deployer == DEPLOYER
                        && balance == Sol(ALLOWANCE_LAMPORTS - 1)
                        && required == Sol(ALLOWANCE_LAMPORTS)
            )));
    }

    #[test]
    fn summary_names_the_deployer_and_the_shortfall() {
        let plan = testing::plan(RESERVE_LAMPORTS);

        let funding = Funding::estimate(&funded(1), &plan, &binaries(&plan), &DEPLOYER).unwrap();

        goldie::assert!(funding.to_string());
    }

    #[test]
    fn sol_shows_nine_decimals() {
        assert_eq!(Sol(1_500_000_001).to_string(), "1.500000001 SOL");
        assert_eq!(Sol(0).to_string(), "0.000000000 SOL");
    }

    #[test]
    fn reserve_top_up_is_at_least_rent_exempt() {
        let chain = RecordingChain::default();
        let reserve = release_address(HYPER_PROVER);

        assert_eq!(reserve_top_up(&chain, &reserve, 0).unwrap(), 0);
        assert_eq!(reserve_top_up(&chain, &reserve, 1).unwrap(), rent(0));
    }
}
