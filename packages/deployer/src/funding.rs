use std::collections::BTreeMap;
use std::fmt;

use layerzero_prover::state::Peer;
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;

use crate::chain::{self, Chain};
use crate::classify::Status;
use crate::plan::{Action, Plan, HYPER_PROVER, LAYERZERO_PROVER};
use crate::{idl, layerzero, layerzero_state, transaction};

/// The configs', `Store`'s and lookup table's rent, `solana-verify`'s verification PDAs, and the
/// unpriced verify and finalize transactions. Each is far below this for any input set that fits
/// a transaction.
const ALLOWANCE_LAMPORTS: u64 = 200_000_000;
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
const MICRO_LAMPORTS_PER_LAMPORT: u128 = 1_000_000;
const SIGNATURE_FEE_LAMPORTS: u64 = 5_000;
/// Bounds for `solana program deploy`. Measured with Solana CLI 3.1.15: buffer writes of about
/// 956 bytes, each with a simulated 2,670 compute-unit limit, plus buffer creation and the final
/// deploy, each signed by the deployer and a second keypair. The bounds leave room for other CLI
/// versions.
const DEPLOY_WRITE_BYTES: usize = 900;
const DEPLOY_COMPUTE_UNIT_LIMIT: u64 = 10_000;
const DEPLOY_SETUP_TRANSACTIONS: u64 = 2;

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
    #[error("the {LAYERZERO_PROVER} reserve will hold {reserve} but its path setup needs {required}; raise layerzero_reserve_lamports")]
    InsufficientLayerZeroReserve { reserve: Sol, required: Sol },
}

/// What a run takes from the deployer, against what it holds. Not part of the plan hash: every
/// fee moves the balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Funding {
    deployer: Pubkey,
    balance: Sol,
    /// Program and program-data rent of every program the run deploys; locked for good once
    /// the program is final. It also covers each buffer: `solana program deploy` writes one at a
    /// time, and loader-v3's `DeployWithMaxDataLen` drains it back to the payer before charging
    /// the program data, which is larger. The balance never dips below the final spend.
    program_rent: Sol,
    /// Rent the canonical IDL accounts still lack, plus each replacement's scratch buffer, which
    /// comes back once it is copied over.
    idl_rent: Sol,
    reserve_top_ups: Sol,
    compute_unit_price: u64,
    /// Signature and priority fees of every deploy, `apply` and IDL transaction at
    /// `compute_unit_price`.
    transaction_fees: Sol,
    /// The LayerZero reserve once topped up; it pays the endpoint and ULN accounts of every path.
    layerzero_reserve: Sol,
    layerzero_path_rent: Sol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sol(u64);

impl Funding {
    pub fn estimate(
        chain: &impl Chain,
        plan: &Plan,
        binaries: &BTreeMap<String, Vec<u8>>,
        idls: &BTreeMap<String, Vec<u8>>,
        deployer: &Pubkey,
    ) -> Result<Self, Error> {
        let rent = Rent::default();
        // A final program's IDL can no longer change, so the IDL step leaves it alone.
        let (idl_rent, idl_transactions) = plan
            .programs
            .iter()
            .filter(|(_, program)| program.state.status != Status::Live)
            .filter_map(|(name, _)| Some((plan.release.programs[name].address, idls.get(name)?)))
            .map(|(address, idl)| idl::cost(chain, &address, idl, &rent))
            .try_fold((0u64, 0u64), |(lamports, transactions), cost| {
                let (rent, sent) = cost?;

                Ok::<_, chain::Error>((lamports.saturating_add(rent), transactions + sent))
            })?;
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
        let reserve = |program, seed, requested_lamports| {
            if !plan.selected.contains(program) {
                return Ok::<_, chain::Error>((0, 0));
            }
            let address = plan.release.programs[program].address;
            let reserve = Pubkey::find_program_address(&[seed], &address).0;
            let balance = lamports(chain, &reserve)?;

            Ok::<_, chain::Error>((balance, top_up(balance, requested_lamports)))
        };
        let (_, hyper_top_up) = reserve(
            HYPER_PROVER,
            hyper_prover::state::PDA_PAYER_SEED,
            plan.inputs.hyper_reserve_lamports,
        )?;
        let (layerzero_balance, layerzero_top_up) = reserve(
            LAYERZERO_PROVER,
            layerzero_prover::state::PDA_PAYER_SEED,
            plan.inputs.layerzero_reserve_lamports,
        )?;

        Ok(Self {
            deployer: *deployer,
            balance: Sol(lamports(chain, deployer)?),
            program_rent: Sol(program_rent),
            idl_rent: Sol(idl_rent),
            reserve_top_ups: Sol(hyper_top_up + layerzero_top_up),
            compute_unit_price: plan.inputs.compute_unit_price,
            transaction_fees: Sol(transaction_fees(plan, &deployed_lengths, idl_transactions)),
            layerzero_reserve: Sol(layerzero_balance + layerzero_top_up),
            layerzero_path_rent: Sol(layerzero_path_rent(plan)),
        })
    }

    pub fn require(&self) -> Result<(), Error> {
        if self.balance < self.required() {
            return Err(Error::InsufficientBalance {
                deployer: self.deployer,
                balance: self.balance,
                required: self.required(),
            });
        }

        match self.layerzero_reserve >= self.layerzero_reserve_required() {
            true => Ok(()),
            false => Err(Error::InsufficientLayerZeroReserve {
                reserve: self.layerzero_reserve,
                required: self.layerzero_reserve_required(),
            }),
        }
    }

    /// The path rent, and the reserve must stay rent-exempt after paying it.
    fn layerzero_reserve_required(&self) -> Sol {
        match self.layerzero_path_rent.0 {
            0 => Sol(0),
            rent => Sol(rent + Rent::default().minimum_balance(0)),
        }
    }

    fn required(&self) -> Sol {
        Sol([
            self.program_rent.0,
            self.idl_rent.0,
            self.reserve_top_ups.0,
            self.transaction_fees.0,
            ALLOWANCE_LAMPORTS,
        ]
        .into_iter()
        .fold(0, u64::saturating_add))
    }
}

/// What `apply` transfers to reach `requested_lamports` in `reserve`; it never takes any back.
pub fn reserve_top_up(
    chain: &impl Chain,
    reserve: &Pubkey,
    requested_lamports: u64,
) -> Result<u64, chain::Error> {
    Ok(top_up(lamports(chain, reserve)?, requested_lamports))
}

fn top_up(balance: u64, requested_lamports: u64) -> u64 {
    target_lamports(requested_lamports).saturating_sub(balance)
}

/// An upper bound: every count and compute-unit limit is the most the step can use.
fn transaction_fees(plan: &Plan, deployed_lengths: &[usize], idl_transactions: u64) -> u64 {
    let price = plan.inputs.compute_unit_price;
    let deploys = deployed_lengths.iter().map(|length| {
        let writes = length.div_ceil(DEPLOY_WRITE_BYTES) as u64;

        fee(price, DEPLOY_COMPUTE_UNIT_LIMIT, 1).saturating_mul(writes)
            + fee(price, DEPLOY_COMPUTE_UNIT_LIMIT, 2) * DEPLOY_SETUP_TRANSACTIONS
    });
    let peers: Vec<Peer> = plan.inputs.layerzero_peers.iter().map(Into::into).collect();
    // IDL transactions go through `transaction::signed` too.
    let inits = plan
        .programs
        .iter()
        .filter(|(name, program)| {
            name.as_str() != LAYERZERO_PROVER && program.actions.contains(&Action::Init)
        })
        .count() as u64;
    let reserves = [HYPER_PROVER, LAYERZERO_PROVER]
        .into_iter()
        .filter(|name| plan.selected.contains(*name))
        .count() as u64;
    let layerzero_setup = if plan
        .programs
        .get(LAYERZERO_PROVER)
        .is_some_and(|program| program.actions.contains(&Action::Init))
    {
        layerzero::setup_transactions(&peers)
    } else {
        0
    };
    let applies = inits + reserves + layerzero_setup + idl_transactions;
    // What `transaction::signed` charges each `apply` and IDL transaction, with the deployer as
    // sole signer.
    let apply_fee = transaction::priority_fee(price).saturating_add(SIGNATURE_FEE_LAMPORTS);

    deploys
        .chain([apply_fee.saturating_mul(applies)])
        .fold(0, u64::saturating_add)
}

/// Signature fees plus the priority fee, `price` micro-lamports for each of `limit` units.
fn fee(price: u64, limit: u64, signatures: u64) -> u64 {
    let priority = (u128::from(price) * u128::from(limit)).div_ceil(MICRO_LAMPORTS_PER_LAMPORT);

    u64::try_from(priority)
        .unwrap_or(u64::MAX)
        .saturating_add(SIGNATURE_FEE_LAMPORTS * signatures)
}

/// Only a run that initializes the LayerZero prover creates path accounts.
fn layerzero_path_rent(plan: &Plan) -> u64 {
    if !plan
        .programs
        .get(LAYERZERO_PROVER)
        .is_some_and(|program| program.actions.contains(&Action::Init))
    {
        return 0;
    }

    plan.inputs
        .layerzero_peers
        .iter()
        .map(|peer| {
            let live = plan
                .live_configs
                .layerzero_paths
                .iter()
                .find(|path| path.eid == peer.eid);

            layerzero_state::path_setup_rent(live)
        })
        .sum()
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
            idl_rent,
            reserve_top_ups,
            compute_unit_price,
            transaction_fees,
            layerzero_reserve,
            layerzero_path_rent,
        } = self;
        let required = self.required();
        let reserve_required = self.layerzero_reserve_required();
        let shortfalls: Vec<String> = [
            (balance < &required).then(|| {
                format!(
                    "**Short by {}**: fund `{deployer}`, then plan again.",
                    Sol(required.0 - balance.0)
                )
            }),
            (layerzero_reserve < &reserve_required).then(|| {
                format!(
                    "**LayerZero reserve short by {}**: raise `layerzero_reserve_lamports` to at least {}.",
                    Sol(reserve_required.0 - layerzero_reserve.0),
                    reserve_required.0
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        let verdict = match shortfalls.is_empty() {
            true => "Funded.".to_owned(),
            false => shortfalls.join("\n\n"),
        };

        write!(
            formatter,
            "### Deployer funding\n\n\
             | | Amount |\n|---|---|\n\
             | Program rent (locked once final) | {program_rent} |\n\
             | Canonical IDL rent (a replacement's scratch buffer comes back) | {idl_rent} |\n\
             | Reserve top-ups | {reserve_top_ups} |\n\
             | Transaction fees at {compute_unit_price} micro-lamports per compute unit | {transaction_fees} |\n\
             | Account rent and unpriced fees allowance | {} |\n\
             | **Required** | {required} |\n\
             | Balance of `{deployer}` | {balance} |\n\
             | LayerZero path rent (paid by its reserve) | {layerzero_path_rent} |\n\
             | LayerZero reserve after top-up | {layerzero_reserve} |\n\n\
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
    use crate::layerzero_state::Path;
    use crate::plan::{AGGREGATOR_PROVER, CCTP_PROVER, LOCAL_PROVER, POLYMER_PROVER};
    use crate::testing::{self, pda, release_address, RecordingChain};

    const DEPLOYER: Pubkey = Pubkey::new_from_array([7; 32]);
    const RESERVE_LAMPORTS: u64 = 5_000_000;
    /// `testing::plan` has one LayerZero peer: 6 + 5 `apply` transactions, one signature each.
    const UNPRICED_APPLY_FEES: u64 = 11 * 5_000;
    const FIXED: u64 = UNPRICED_APPLY_FEES + ALLOWANCE_LAMPORTS;

    /// No release IDLs: the IDL step adds nothing.
    fn no_idls() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::new()
    }

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

        let funding =
            Funding::estimate(&funded(0), &plan, &binaries(&plan), &no_idls(), &DEPLOYER).unwrap();

        assert_eq!(funding.required(), Sol(2 * RESERVE_LAMPORTS + FIXED));
    }

    /// An absent canonical IDL needs its full rent and its allocate, writes and initialize; one
    /// already equal costs nothing, and a final program's is left alone.
    #[test]
    fn the_idl_step_adds_the_rent_and_transactions_still_needed() {
        let mut plan = testing::plan(RESERVE_LAMPORTS);
        plan.programs.get_mut(LOCAL_PROVER).unwrap().state.status = Status::Live;
        let idl = |name: &str| format!(r#"{{"name":"{name}"}}"#).into_bytes();
        let idls: BTreeMap<String, Vec<u8>> = plan
            .programs
            .keys()
            .map(|name| (name.clone(), idl(name)))
            .collect();
        let mut chain = funded(0);
        let uploaded = plan.release.programs[POLYMER_PROVER].address;
        chain.accounts.insert(
            idl::canonical_address(&uploaded),
            idl::uploaded_account(&uploaded, &idl(POLYMER_PROVER)),
        );
        let base = Funding::estimate(&funded(0), &plan, &binaries(&plan), &no_idls(), &DEPLOYER)
            .unwrap()
            .required();

        let funding = Funding::estimate(&chain, &plan, &binaries(&plan), &idls, &DEPLOYER).unwrap();

        // Four programs upload: every one but the final local prover and the matching polymer
        // prover, each in one write.
        let uploads: u64 = [
            HYPER_PROVER,
            AGGREGATOR_PROVER,
            LAYERZERO_PROVER,
            CCTP_PROVER,
        ]
        .iter()
        .map(|name| rent(idl::HEADER_LEN + idl::compress(&idl(name)).len()))
        .sum();
        assert_eq!(funding.idl_rent, Sol(uploads));
        assert_eq!(funding.required(), Sol(base.0 + uploads + 4 * 4 * 5_000));
    }

    #[test]
    fn deployed_programs_add_only_their_rent() {
        let mut plan = testing::plan(0);
        plan.programs
            .values_mut()
            .for_each(|program| program.actions.insert(0, Action::Deploy));
        let mut binaries = binaries(&plan);
        binaries.insert(LAYERZERO_PROVER.into(), vec![0; 3_000]);

        let funding =
            Funding::estimate(&funded(0), &plan, &binaries, &no_idls(), &DEPLOYER).unwrap();

        assert_eq!(
            funding.required(),
            Sol(5 * (rent(45 + 1_000) + rent(36))
                + rent(45 + 3_000)
                + rent(36)
                + 5 * (2 * 5_000 + 2 * 10_000)
                + (4 * 5_000 + 2 * 10_000)
                + FIXED)
        );
    }

    /// Why `program_rent` needs no buffer term: before its deploy instruction drains it, a
    /// program's buffer holds less than that program's own rent.
    #[test]
    fn a_buffer_never_costs_more_than_its_program() {
        // The last length is the largest program data an account can hold (10 MiB).
        for length in [0, 1, 1_000, 455_808, 10 * 1024 * 1024 - 45] {
            assert!(
                rent(UpgradeableLoaderState::size_of_buffer(length))
                    < rent(UpgradeableLoaderState::size_of_programdata(length))
                        + rent(UpgradeableLoaderState::size_of_program()),
                "{length}"
            );
        }
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

        let funding =
            Funding::estimate(&chain, &plan, &binaries(&plan), &no_idls(), &DEPLOYER).unwrap();

        assert_eq!(funding.required(), Sol(1 + FIXED));
    }

    #[test]
    fn require_rejects_a_balance_below_the_estimate() {
        let mut plan = testing::plan(0);
        plan.programs
            .get_mut(LAYERZERO_PROVER)
            .unwrap()
            .actions
            .clear();
        let estimate = |lamports| {
            Funding::estimate(
                &funded(lamports),
                &plan,
                &binaries(&plan),
                &no_idls(),
                &DEPLOYER,
            )
            .unwrap()
        };

        let required = estimate(0).required().0;
        assert!(estimate(required).require().is_ok());
        assert!(estimate(required - 1).require().is_err_and(|error| matches!(
            error,
            Error::InsufficientBalance { deployer, balance, required: needed }
                if deployer == DEPLOYER && balance == Sol(required - 1) && needed == Sol(required)
        )));
    }

    #[test]
    fn priority_fees_scale_with_the_compute_unit_price_and_every_compute_unit() {
        let mut plan = testing::plan(0);
        plan.programs
            .values_mut()
            .for_each(|program| program.actions.insert(0, Action::Deploy));
        let mut fees = |price| {
            plan.inputs.compute_unit_price = price;
            Funding::estimate(&funded(0), &plan, &binaries(&plan), &no_idls(), &DEPLOYER)
                .unwrap()
                .transaction_fees
        };
        let unpriced = fees(0);

        let priced = fees(1_000_000);

        // Six 1,000-byte programs: 2 writes and 2 setup transactions each, at 10,000 units.
        let deploy_units = 6 * 4 * 10_000;
        let apply_units = 11 * 1_400_000;
        assert_eq!(priced, Sol(unpriced.0 + deploy_units + apply_units));
        assert_eq!(fees(u64::MAX), Sol(u64::MAX));
    }

    #[test]
    fn a_compute_unit_price_the_balance_cannot_pay_is_refused() {
        let mut plan = testing::plan(0);
        plan.programs
            .get_mut(LAYERZERO_PROVER)
            .unwrap()
            .actions
            .clear();
        let required =
            Funding::estimate(&funded(0), &plan, &binaries(&plan), &no_idls(), &DEPLOYER)
                .unwrap()
                .required()
                .0;
        let chain = funded(required);
        let estimate = |plan: &Plan| {
            Funding::estimate(&chain, plan, &binaries(plan), &no_idls(), &DEPLOYER).unwrap()
        };
        assert!(estimate(&plan).require().is_ok());

        plan.inputs.compute_unit_price = 1;

        assert!(estimate(&plan)
            .require()
            .is_err_and(|error| matches!(error, Error::InsufficientBalance { .. })));
    }

    #[test]
    fn layerzero_reserve_must_cover_every_path_account_and_stay_rent_exempt() {
        let path_rent = [25, 2_061, 41, 82, 1_088, 1_052]
            .map(rent)
            .iter()
            .sum::<u64>();
        let required = path_rent + rent(0);
        let estimate = |reserve_lamports| {
            let plan = testing::plan(reserve_lamports);
            Funding::estimate(
                &funded(u64::MAX / 2),
                &plan,
                &binaries(&plan),
                &no_idls(),
                &DEPLOYER,
            )
            .unwrap()
        };

        assert!(estimate(required).require().is_ok());
        assert!(estimate(required - 1)
            .require()
            .is_err_and(|error| matches!(
                error,
                Error::InsufficientLayerZeroReserve { reserve, required: needed }
                    if reserve == Sol(required - 1) && needed == Sol(required)
            )));
    }

    #[test]
    fn layerzero_reserve_needs_rent_only_for_path_accounts_not_yet_created() {
        let mut plan = testing::plan(0);
        let peer = &plan.inputs.layerzero_peers[0];
        let expected = Path::expected(&layerzero_prover::ID, &peer.into(), &peer.path);
        plan.live_configs.layerzero_paths = vec![Path {
            send: None,
            receive: None,
            ..expected.clone()
        }];
        let reserve_required = |plan: &Plan| {
            Funding::estimate(&funded(0), plan, &binaries(plan), &no_idls(), &DEPLOYER)
                .unwrap()
                .layerzero_reserve_required()
        };

        assert_eq!(
            reserve_required(&plan),
            Sol(rent(1_088) + rent(1_052) + rent(0))
        );
        plan.live_configs.layerzero_paths = vec![expected];
        assert_eq!(reserve_required(&plan), Sol(0));
    }

    #[test]
    fn summary_names_the_deployer_and_the_shortfall() {
        let plan = testing::plan(RESERVE_LAMPORTS);

        let funding =
            Funding::estimate(&funded(1), &plan, &binaries(&plan), &no_idls(), &DEPLOYER).unwrap();

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
