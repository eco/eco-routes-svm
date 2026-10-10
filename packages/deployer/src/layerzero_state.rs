use layerzero_prover::instructions::{required_alt_addresses, PathConfig};
use layerzero_prover::layerzero::{self, ExecutorConfig, UlnConfig, ENDPOINT_ID};
use layerzero_prover::state::{Peer, Store, STORE_SEED};
use solana_address_lookup_table_interface::state::AddressLookupTable;
use solana_sdk::account::Account;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk_ids::address_lookup_table;

use crate::chain::Chain;
use crate::config::{Error, Mismatch};
use crate::plan::LAYERZERO_PROVER;
use crate::uln::{self, ReceiveConfig, SendConfig};

/// Sizes, discriminator included, of the accounts `pda_payer` pays for as delegate, per
/// LayerZero-v2@9c741e7f: `init_path` creates the endpoint `Nonce`, `PendingInboundNonce` (256
/// nonces), `SendLibraryConfig` and `ReceiveLibraryConfig`; `set_path_config` creates ULN302's
/// `SendConfig` and `ReceiveConfig` (16 DVNs each list). Devnet holds thousands of each size.
const INIT_PATH_ACCOUNT_SIZES: [usize; 4] = [25, 2061, 41, 82];
const SET_PATH_CONFIG_ACCOUNT_SIZES: [usize; 2] = [1088, 1052];

/// The endpoint and ULN accounts a peer's path creates, derived for a deployment at `program`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAccounts {
    pub nonce: Pubkey,
    pub send_config: Pubkey,
    pub receive_config: Pubkey,
}

/// What is on chain for one peer's path. `send` and `receive` are the decoded ULN302 per-OApp
/// configs, `None` while their accounts do not exist.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub eid: u32,
    pub accounts: PathAccounts,
    pub nonce: bool,
    pub send: Option<SendConfig>,
    pub receive: Option<ReceiveConfig>,
}

/// The lookup table recorded in the `Store`, and what is wrong with it, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alt {
    pub address: Pubkey,
    pub fault: Option<AltFault>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AltFault {
    Invalid,
    Unfrozen,
    Deactivated,
    Incomplete { missing: Pubkey },
}

/// Rent `pda_payer` still has to pay to set up a peer's path, given what is on chain for it.
pub fn path_setup_rent(live: Option<&Path>) -> u64 {
    let rent = Rent::default();
    let nonce_exists = live.is_some_and(|path| path.nonce);
    let configs_exist = live.is_some_and(|path| path.send.is_some() || path.receive.is_some());

    [
        (nonce_exists, INIT_PATH_ACCOUNT_SIZES.as_slice()),
        (configs_exist, SET_PATH_CONFIG_ACCOUNT_SIZES.as_slice()),
    ]
    .into_iter()
    .filter(|(exists, _)| !exists)
    .flat_map(|(_, sizes)| sizes)
    .map(|size| rent.minimum_balance(*size))
    .sum()
}

pub fn store_address(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[STORE_SEED], program).0
}

pub fn path_accounts(program: &Pubkey, peer: &Peer) -> PathAccounts {
    let store = store_address(program);

    PathAccounts {
        nonce: layerzero::nonce_pda(&store, peer.eid, &peer.address).0,
        send_config: layerzero::uln_send_config_pda(peer.eid, &store).0,
        receive_config: layerzero::uln_receive_config_pda(peer.eid, &store).0,
    }
}

impl Path {
    /// The state a fully set up path has when `path` was applied.
    pub fn expected(program: &Pubkey, peer: &Peer, path: &PathConfig) -> Self {
        Self {
            eid: peer.eid,
            accounts: path_accounts(program, peer),
            nonce: true,
            send: Some(SendConfig {
                uln: path.send_uln.clone(),
                executor: path.executor.clone(),
            }),
            receive: Some(ReceiveConfig {
                uln: path.receive_uln.clone(),
            }),
        }
    }

    /// Fails when a config that exists differs from `expected`; absent configs pass.
    pub fn conflict(&self, expected: &Self) -> Result<(), Mismatch> {
        let send_agrees = self.send.is_none() || self.send == expected.send;
        let receive_agrees = self.receive.is_none() || self.receive == expected.receive;

        match send_agrees && receive_agrees {
            true => Ok(()),
            false => Err(Mismatch {
                program: LAYERZERO_PROVER,
                expected: expected.describe(),
                actual: self.describe(),
            }),
        }
    }

    pub fn describe(&self) -> String {
        let send = self.send.as_ref().map_or("absent".to_owned(), |send| {
            format!(
                "{}, executor {}",
                describe_uln(&send.uln),
                describe_executor(&send.executor)
            )
        });
        let receive = self
            .receive
            .as_ref()
            .map_or("absent".to_owned(), |receive| describe_uln(&receive.uln));

        format!(
            "eid {} nonce {} send [{send}] receive [{receive}]",
            self.eid, self.nonce
        )
    }
}

impl Alt {
    pub fn describe(&self) -> String {
        match &self.fault {
            None => format!("{} ok", self.address),
            Some(fault) => format!("{} {fault:?}", self.address),
        }
    }
}

pub fn read_path(chain: &impl Chain, program: &Pubkey, peer: &Peer) -> Result<Path, Error> {
    let accounts = path_accounts(program, peer);
    let nonce = chain
        .account(&accounts.nonce)?
        .is_some_and(|account| account.owner == ENDPOINT_ID && !account.data.is_empty());

    Ok(Path {
        eid: peer.eid,
        nonce,
        send: uln::read_send(chain, &accounts.send_config)?,
        receive: uln::read_receive(chain, &accounts.receive_config)?,
        accounts,
    })
}

pub fn read_paths(chain: &impl Chain, program: &Pubkey, store: &Store) -> Result<Vec<Path>, Error> {
    store
        .peers
        .iter()
        .map(|peer| read_path(chain, program, peer))
        .collect()
}

pub fn read_alt(chain: &impl Chain, store: &Store) -> Result<Option<Alt>, Error> {
    let address = store.alt;
    if address == Pubkey::default() {
        return Ok(None);
    }
    let fault = alt_fault(chain.account(&address)?, store);

    Ok(Some(Alt { address, fault }))
}

/// After finalization nothing can replace or repair the table, so it must be frozen, never
/// deactivated, and hold every address `lz_receive` needs.
fn alt_fault(account: Option<Account>, store: &Store) -> Option<AltFault> {
    let Some(account) = account.filter(|account| account.owner == address_lookup_table::id())
    else {
        return Some(AltFault::Invalid);
    };
    let Ok(table) = AddressLookupTable::deserialize(&account.data) else {
        return Some(AltFault::Invalid);
    };
    if table.meta.authority.is_some() {
        return Some(AltFault::Unfrozen);
    }
    if table.meta.deactivation_slot != u64::MAX {
        return Some(AltFault::Deactivated);
    }

    required_alt_addresses(store)
        .into_iter()
        .find(|address| !table.addresses.contains(address))
        .map(|missing| AltFault::Incomplete { missing })
}

fn describe_uln(uln: &UlnConfig) -> String {
    let dvns = |dvns: &[Pubkey]| {
        dvns.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    };

    format!(
        "confirmations {} required {}/[{}] optional {}/{}/[{}]",
        uln.confirmations,
        uln.required_dvn_count,
        dvns(&uln.required_dvns),
        uln.optional_dvn_count,
        uln.optional_dvn_threshold,
        dvns(&uln.optional_dvns)
    )
}

fn describe_executor(executor: &ExecutorConfig) -> String {
    format!(
        "{} max size {}",
        executor.executor, executor.max_message_size
    )
}
