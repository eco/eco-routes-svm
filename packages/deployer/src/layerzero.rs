use std::collections::BTreeSet;

use anchor_lang::{system_program, InstructionData, ToAccountMetas};
use layerzero_prover::instructions::{required_alt_addresses, InitArgs, PathConfig};
use layerzero_prover::layerzero::{
    self, ExecutorConfig, UlnConfig, ENDPOINT_ID, LZ_RECEIVE_TYPES_SEED, ULN_ID,
};
use layerzero_prover::state::{Peer, Store, PDA_PAYER_SEED};
use solana_address_lookup_table_interface::instruction::{
    create_lookup_table, extend_lookup_table, freeze_lookup_table,
};
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;

use crate::chain::{self, Chain};
use crate::config::{self, Configs};
use crate::inputs::LayerZeroPeer;
use crate::layerzero_state::{self, Path};
use crate::plan::{Plan, LAYERZERO_PROVER};
use crate::transaction;

const ALT_EXTEND_CHUNK: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error(transparent)]
    Mismatch(#[from] config::Mismatch),
    #[error(transparent)]
    Transaction(#[from] transaction::Error),
    #[error("{LAYERZERO_PROVER} is not in the plan")]
    UnplannedProgram,
    #[error("{LAYERZERO_PROVER}: eid {eid} has only one of its ULN configs, and init_config cannot run twice; close it (and the aggregator) with close.yml, raise its salt in scripts/program-salts.json and release again")]
    PartialPathConfig { eid: u32 },
    #[error("{LAYERZERO_PROVER} {transaction} needs {size} bytes, over the {limit}-byte transaction limit; use fewer DVNs")]
    TransactionTooLarge {
        transaction: String,
        size: usize,
        limit: usize,
    },
    #[error("{LAYERZERO_PROVER} store {address} does not exist after init")]
    StoreMissing { address: Pubkey },
    #[error("{LAYERZERO_PROVER}: {role} {address} {reason}; ULN302 names each worker by its config PDA under the worker's program ([\"ExecutorConfig\"] for the executor, [\"DvnConfig\"] for a DVN), never by its program ID")]
    NotAWorkerConfig {
        role: &'static str,
        address: Pubkey,
        reason: &'static str,
    },
}

/// One LayerZero setup action; `signature` is `None` when it was already done.
#[derive(Debug, Clone)]
pub struct Action {
    pub name: &'static str,
    pub signature: Option<Signature>,
}

/// Rejects a setup whose `init` (every peer) or any `set_path_config` would not fit one
/// transaction as `apply` sends it: deployer as sole signer and fee payer, built with the real
/// instructions at `program`.
pub fn check_transaction_sizes(program: &Pubkey, peers: &[LayerZeroPeer]) -> Result<(), Error> {
    let deployer = Pubkey::new_from_array([1; 32]);
    let init = (
        "init".to_owned(),
        init_instruction(program, &deployer, peers.iter().map(Into::into).collect()),
    );
    let set_path_configs = peers.iter().map(|peer| {
        (
            format!("set_path_config for eid {}", peer.eid),
            set_path_config_instruction(program, &deployer, peer.eid, &peer.path),
        )
    });

    std::iter::once(init)
        .chain(set_path_configs)
        .try_for_each(|(name, instruction)| {
            let size = transaction::size(&deployer, &[instruction])?;

            match size <= transaction::MAX_SIZE {
                true => Ok(()),
                false => Err(Error::TransactionTooLarge {
                    transaction: name,
                    size,
                    limit: transaction::MAX_SIZE,
                }),
            }
        })
}

pub const EXECUTOR_ROLE: &str = "executor";
pub const DVN_ROLE: &str = "DVN";
/// The seeds LayerZero's workers pin their config account to (`quote.rs` of the executor and
/// the DVN at LayerZero-v2@9c741e7f); ULN302 hands that account to its owner's `quote`.
const EXECUTOR_CONFIG_SEED: &[u8] = b"ExecutorConfig";
const DVN_CONFIG_SEED: &[u8] = b"DvnConfig";

/// Rejects a worker in a path `apply` will write: those of input peers with no ULN config on
/// chain yet. A path already configured is whatever the chain holds (`verify_configs` makes the
/// inputs equal it), and once `layerzero_prover` is final nothing can change it, so checking it
/// here would only block every later plan of the release; finalize checks the live paths.
pub fn check_workers(chain: &impl Chain, plan: &Plan) -> Result<(), Error> {
    let configured: Vec<u32> = plan
        .live_configs
        .layerzero_paths
        .iter()
        .filter(|path| path.send.is_some() || path.receive.is_some())
        .map(|path| path.eid)
        .collect();
    let workers = plan
        .inputs
        .layerzero_peers
        .iter()
        .filter(|peer| !configured.contains(&peer.eid))
        .flat_map(|peer| {
            let PathConfig {
                send_uln,
                receive_uln,
                executor,
            } = &peer.path;

            workers(executor, [send_uln, receive_uln])
        });

    check_addresses(chain, workers)
}

/// Rejects a worker in a live path, before finalize locks it in.
pub fn check_live_workers(chain: &impl Chain, paths: &[Path]) -> Result<(), Error> {
    let workers = paths.iter().flat_map(|path| {
        let send = path.send.as_ref();
        let ulns = send
            .map(|send| &send.uln)
            .into_iter()
            .chain(path.receive.as_ref().map(|receive| &receive.uln));

        send.map(|send| workers(&send.executor, ulns))
            .into_iter()
            .flatten()
    });

    check_addresses(chain, workers)
}

/// The executor and every DVN, required and optional, of `ulns`.
fn workers<'a>(
    executor: &ExecutorConfig,
    ulns: impl IntoIterator<Item = &'a UlnConfig>,
) -> Vec<(&'static str, Pubkey)> {
    let dvns = ulns
        .into_iter()
        .flat_map(|uln| uln.required_dvns.iter().chain(&uln.optional_dvns))
        .map(|dvn| (DVN_ROLE, *dvn));

    std::iter::once((EXECUTOR_ROLE, executor.executor))
        .chain(dvns)
        .collect()
}

/// Checks each distinct worker once: the same DVNs repeat across peers and both ULN configs.
fn check_addresses(
    chain: &impl Chain,
    workers: impl IntoIterator<Item = (&'static str, Pubkey)>,
) -> Result<(), Error> {
    workers
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .try_for_each(|(role, address)| check_worker(chain, role, &address))
}

/// A worker config is an account at its role's PDA under an executable program, its owner.
fn check_worker(chain: &impl Chain, role: &'static str, address: &Pubkey) -> Result<(), Error> {
    let seed = match role {
        EXECUTOR_ROLE => EXECUTOR_CONFIG_SEED,
        _ => DVN_CONFIG_SEED,
    };
    let reason = match chain.account(address)? {
        None => Some("does not exist"),
        Some(account) if account.executable => Some("is a program"),
        Some(account) if Pubkey::find_program_address(&[seed], &account.owner).0 != *address => {
            Some("is not its owner's config PDA for that role")
        }
        Some(account) => match chain.account(&account.owner)? {
            Some(owner) if owner.executable => None,
            _ => Some("is not owned by a program"),
        },
    };

    reason.map_or(Ok(()), |reason| {
        Err(Error::NotAWorkerConfig {
            role,
            address: *address,
            reason,
        })
    })
}

/// Every transaction `apply` sends for a LayerZero setup of `peers` from scratch: `init`,
/// `init_path` and `set_path_config` per peer, and the lookup table's.
pub fn setup_transactions(peers: &[Peer]) -> u64 {
    let store = Store::new(peers.to_vec()).expect("planned peers must form a valid store");
    let extends = required_alt_addresses(&store)
        .len()
        .div_ceil(ALT_EXTEND_CHUNK);
    // `create` shares the first extend's transaction and `freeze` shares `set_alt`'s.
    let lookup_table = extends + 1;

    (1 + 2 * peers.len() + lookup_table) as u64
}

/// Init, paths and lookup table in spec order, calling `observe` with each action as soon as it
/// has landed; every action reads before it writes.
pub fn apply<C: Chain>(
    chain: &mut C,
    plan: &Plan,
    deployer: &Keypair,
    observe: &mut impl FnMut(&Action),
) -> Result<(), Error> {
    let program = plan
        .programs
        .get(LAYERZERO_PROVER)
        .map(|planned| planned.state.address)
        .ok_or(Error::UnplannedProgram)?;
    let peers: Vec<(Peer, PathConfig)> = plan
        .inputs
        .layerzero_peers
        .iter()
        .map(|peer| (peer.into(), peer.path.clone()))
        .collect();
    let mut landed = |name, signature| observe(&Action { name, signature });

    landed("init", init(chain, plan, &program, deployer, &peers)?);
    peers.iter().try_for_each(|(peer, _)| {
        landed("init_path", init_path(chain, &program, deployer, peer)?);

        Ok::<_, Error>(())
    })?;
    peers.iter().try_for_each(|(peer, path)| {
        let signature = set_path_config(chain, &program, deployer, peer, path)?;
        landed("set_path_config", signature);

        Ok::<_, Error>(())
    })?;
    landed("set_alt", set_alt(chain, &program, deployer)?);

    Ok(())
}

fn init(
    chain: &mut impl Chain,
    plan: &Plan,
    program: &Pubkey,
    deployer: &Keypair,
    peers: &[(Peer, PathConfig)],
) -> Result<Option<Signature>, Error> {
    let live = Configs {
        layerzero_peers: config::layerzero_store(chain, program)?.map(|store| store.peers),
        ..Configs::default()
    };
    live.conflict(&plan.expected_configs)?;
    if !live.is_absent() {
        return Ok(None);
    }
    let peers = peers.iter().map(|(peer, _)| *peer).collect();

    send(
        chain,
        deployer,
        &[init_instruction(program, &deployer.pubkey(), peers)],
    )
}

fn init_path(
    chain: &mut impl Chain,
    program: &Pubkey,
    deployer: &Keypair,
    peer: &Peer,
) -> Result<Option<Signature>, Error> {
    if layerzero_state::read_path(chain, program, peer)?.nonce {
        return Ok(None);
    }

    send(
        chain,
        deployer,
        &[init_path_instruction(program, &deployer.pubkey(), peer)],
    )
}

fn set_path_config(
    chain: &mut impl Chain,
    program: &Pubkey,
    deployer: &Keypair,
    peer: &Peer,
    path: &PathConfig,
) -> Result<Option<Signature>, Error> {
    let live = layerzero_state::read_path(chain, program, peer)?;
    live.conflict(&Path::expected(program, peer, path))?;
    match (live.send.is_some(), live.receive.is_some()) {
        (true, true) => return Ok(None),
        (false, false) => {}
        _ => return Err(Error::PartialPathConfig { eid: peer.eid }),
    }
    let instruction = set_path_config_instruction(program, &deployer.pubkey(), peer.eid, path);

    send(chain, deployer, &[instruction])
}

/// Creates, fills and freezes a lookup table, then records it. Freezing and recording share a
/// transaction so a frozen table is never left unrecorded. A run that dies before that
/// transaction leaves an unfrozen table the next run cannot find, since only the `Store` points
/// at it; the next run creates a new one and the orphan only wastes rent.
fn set_alt(
    chain: &mut impl Chain,
    program: &Pubkey,
    deployer: &Keypair,
) -> Result<Option<Signature>, Error> {
    let address = layerzero_state::store_address(program);
    let store = config::layerzero_store(chain, program)?.ok_or(Error::StoreMissing { address })?;
    if store.alt != Pubkey::default() {
        return Ok(None);
    }
    let authority = deployer.pubkey();
    let (create, alt) = create_lookup_table(authority, authority, chain.slot()?);
    let required = required_alt_addresses(&store);
    let mut extensions = required
        .chunks(ALT_EXTEND_CHUNK)
        .map(|addresses| extend_lookup_table(alt, authority, Some(authority), addresses.to_vec()));
    let first = vec![
        create,
        extensions
            .next()
            .expect("required lookup table addresses must not be empty"),
    ];
    let last = vec![
        freeze_lookup_table(alt, authority),
        set_alt_instruction(program, &authority, &alt),
    ];

    std::iter::once(first)
        .chain(extensions.map(|extension| vec![extension]))
        .chain([last])
        .try_fold(None, |_, instructions| send(chain, deployer, &instructions))
}

fn send(
    chain: &mut impl Chain,
    deployer: &Keypair,
    instructions: &[Instruction],
) -> Result<Option<Signature>, Error> {
    chain
        .send(instructions, &[deployer])
        .map(Some)
        .map_err(Into::into)
}

fn init_instruction(program: &Pubkey, deployer: &Pubkey, peers: Vec<Peer>) -> Instruction {
    let store = layerzero_state::store_address(program);
    let accounts = layerzero_prover::accounts::Init {
        payer: *deployer,
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        store,
        lz_receive_types: Pubkey::find_program_address(
            &[LZ_RECEIVE_TYPES_SEED, store.as_ref()],
            program,
        )
        .0,
        system_program: system_program::ID,
        endpoint_program: ENDPOINT_ID,
        oapp_registry: layerzero::oapp_registry_pda(&store).0,
        endpoint_event_authority: layerzero::endpoint_event_authority().0,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: layerzero_prover::instruction::Init {
            args: InitArgs { peers },
        }
        .data(),
    }
}

fn init_path_instruction(program: &Pubkey, deployer: &Pubkey, peer: &Peer) -> Instruction {
    let store = layerzero_state::store_address(program);
    let accounts = layerzero_prover::accounts::InitPath {
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        store,
        pda_payer: pda_payer_address(program),
        system_program: system_program::ID,
        endpoint_program: ENDPOINT_ID,
        oapp_registry: layerzero::oapp_registry_pda(&store).0,
        nonce: layerzero::nonce_pda(&store, peer.eid, &peer.address).0,
        pending_nonce: layerzero::pending_nonce_pda(&store, peer.eid, &peer.address).0,
        send_library_config: layerzero::send_library_config_pda(&store, peer.eid).0,
        receive_library_config: layerzero::receive_library_config_pda(&store, peer.eid).0,
        message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0).0,
        endpoint_event_authority: layerzero::endpoint_event_authority().0,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: layerzero_prover::instruction::InitPath { eid: peer.eid }.data(),
    }
}

fn set_path_config_instruction(
    program: &Pubkey,
    deployer: &Pubkey,
    eid: u32,
    config: &PathConfig,
) -> Instruction {
    let store = layerzero_state::store_address(program);
    let accounts = layerzero_prover::accounts::SetPathConfig {
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        store,
        pda_payer: pda_payer_address(program),
        system_program: system_program::ID,
        endpoint_program: ENDPOINT_ID,
        oapp_registry: layerzero::oapp_registry_pda(&store).0,
        message_lib_info: layerzero::message_lib_info_pda(&layerzero::uln_settings_pda().0).0,
        uln_settings: layerzero::uln_settings_pda().0,
        uln_program: ULN_ID,
        uln_send_config: layerzero::uln_send_config_pda(eid, &store).0,
        uln_receive_config: layerzero::uln_receive_config_pda(eid, &store).0,
        uln_default_send_config: layerzero::uln_default_send_config_pda(eid).0,
        uln_default_receive_config: layerzero::uln_default_receive_config_pda(eid).0,
        uln_event_authority: layerzero::uln_event_authority().0,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: layerzero_prover::instruction::SetPathConfig {
            eid,
            config: config.clone(),
        }
        .data(),
    }
}

fn set_alt_instruction(program: &Pubkey, deployer: &Pubkey, alt: &Pubkey) -> Instruction {
    let accounts = layerzero_prover::accounts::SetAlt {
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        store: layerzero_state::store_address(program),
        alt: *alt,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: layerzero_prover::instruction::SetAlt {}.data(),
    }
}

fn pda_payer_address(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PDA_PAYER_SEED], program).0
}

#[cfg(test)]
mod tests {
    use layerzero_prover::layerzero::{ExecutorConfig, UlnConfig};
    use layerzero_prover::state::MAX_PEERS;
    use solana_sdk::account::Account;

    use super::*;
    use crate::inputs::MAX_DVNS;
    use crate::testing::{
        full_setup, plan, release_address, uln_account_data, worker_program, FullSetup,
        RecordingChain, DVN, EXECUTOR,
    };

    /// The fewest required DVNs, with no optional ones, whose `set_path_config` exceeds a
    /// transaction.
    const OVERSIZED_DVNS: u8 = 53;

    fn applied(
        chain: &mut impl Chain,
        plan: &Plan,
        deployer: &Keypair,
    ) -> Result<Vec<Action>, Error> {
        let mut actions = Vec::new();
        apply(chain, plan, deployer, &mut |action| {
            actions.push(action.clone())
        })?;

        Ok(actions)
    }

    /// `required` and `optional` DVNs on both ULN configs; no optional DVNs is NIL.
    fn path(required: u8, optional: u8) -> PathConfig {
        let (optional_dvn_count, optional_dvn_threshold) = match optional {
            0 => (u8::MAX, 0),
            count => (count, 1),
        };
        let uln = UlnConfig {
            confirmations: 15,
            required_dvn_count: required,
            optional_dvn_count,
            optional_dvn_threshold,
            required_dvns: (0..required).map(|_| Pubkey::new_unique()).collect(),
            optional_dvns: (0..optional).map(|_| Pubkey::new_unique()).collect(),
        };

        PathConfig {
            send_uln: uln.clone(),
            receive_uln: uln,
            executor: ExecutorConfig {
                max_message_size: 10_000,
                executor: Pubkey::new_unique(),
            },
        }
    }

    fn with_other_send_executor(setup: &mut FullSetup, plan: &Plan) {
        let send = plan.expected_configs.layerzero_paths[0]
            .send
            .clone()
            .unwrap();
        let mut executor = send.executor;
        executor.max_message_size += 1;
        setup.chain.accounts.insert(
            setup.send_config,
            Account {
                owner: ULN_ID,
                data: uln_account_data("SendConfig", &(255u8, send.uln, executor)),
                ..Account::default()
            },
        );
    }

    #[test]
    fn complete_setup_sends_nothing() {
        let plan = plan(5_000_000);
        let mut setup = full_setup(&plan);

        let actions = applied(&mut setup.chain, &plan, &Keypair::new()).unwrap();

        assert!(actions.iter().all(|action| action.signature.is_none()));
        assert!(setup.chain.sent.is_empty());
    }

    #[test]
    fn existing_path_config_that_differs_is_refused_without_a_write() {
        let plan = plan(5_000_000);
        let mut setup = full_setup(&plan);
        with_other_send_executor(&mut setup, &plan);

        let result = applied(&mut setup.chain, &plan, &Keypair::new());

        assert!(matches!(
            result,
            Err(Error::Mismatch(mismatch)) if mismatch.program == LAYERZERO_PROVER
        ));
        assert!(setup.chain.sent.is_empty());
    }

    #[test]
    fn path_with_only_one_uln_config_is_refused_without_a_write() {
        let plan = plan(5_000_000);
        let mut setup = full_setup(&plan);
        setup.chain.accounts.remove(&setup.receive_config);

        let result = applied(&mut setup.chain, &plan, &Keypair::new());

        assert!(matches!(result, Err(Error::PartialPathConfig { eid: 1 })));
        assert!(setup.chain.sent.is_empty());
    }

    #[test]
    fn setup_transactions_counts_paths_and_lookup_table_extends() {
        let peers = |count: u32| -> Vec<Peer> {
            (1..=count)
                .map(|eid| Peer {
                    eid,
                    address: [1; 32].into(),
                    chain_id: eid.into(),
                })
                .collect()
        };

        // init, 2 per peer, create with the only extend, freeze with set_alt.
        assert_eq!(setup_transactions(&peers(1)), 1 + 2 + 2);
        // init, 2 per peer, three extends of 42 addresses, then set_alt.
        assert_eq!(setup_transactions(&peers(MAX_PEERS as u32)), 1 + 64 + 4);
    }

    #[test]
    fn largest_transactions_fit_one_transaction() {
        let program = Pubkey::new_unique();
        let deployer = Pubkey::new_unique();
        let peers: Vec<Peer> = (1..=MAX_PEERS as u32)
            .map(|eid| Peer {
                eid,
                address: [0; 32].into(),
                chain_id: eid.into(),
            })
            .collect();

        let sizes = [
            init_instruction(&program, &deployer, peers),
            set_path_config_instruction(&program, &deployer, 1, &path(MAX_DVNS, MAX_DVNS)),
        ]
        .map(|instruction| transaction::size(&deployer, &[instruction]).unwrap());

        assert!(
            sizes.iter().all(|size| *size <= transaction::MAX_SIZE),
            "{sizes:?}"
        );
    }

    #[test]
    fn a_path_config_over_the_transaction_limit_is_rejected() {
        let peer = LayerZeroPeer {
            eid: 30184,
            address: "0xAbCdEf0123456789aBcDeF0123456789abcdef01"
                .parse()
                .unwrap(),
            chain_id: 30184,
            path: path(OVERSIZED_DVNS, 0),
        };

        let result = check_transaction_sizes(&Pubkey::new_unique(), &[peer]);

        assert!(
            matches!(
                &result,
                Err(Error::TransactionTooLarge { transaction, size, limit })
                    if transaction == "set_path_config for eid 30184" && *size == 4132 && *limit == 4096
            ),
            "{result:?}"
        );
    }

    fn uln(dvns: Vec<Pubkey>) -> UlnConfig {
        UlnConfig {
            confirmations: 15,
            required_dvn_count: dvns.len() as u8,
            optional_dvn_count: u8::MAX,
            optional_dvn_threshold: 0,
            required_dvns: dvns,
            optional_dvns: vec![],
        }
    }

    /// The fixture's executor and DVN, or the ones given.
    fn path_with(executor: Pubkey, dvn: Pubkey) -> PathConfig {
        PathConfig {
            send_uln: uln(vec![dvn]),
            receive_uln: uln(vec![dvn]),
            executor: ExecutorConfig {
                max_message_size: 10_000,
                executor,
            },
        }
    }

    fn fixture_path() -> PathConfig {
        path_with(EXECUTOR.parse().unwrap(), DVN.parse().unwrap())
    }

    fn check(chain: &RecordingChain, path: &PathConfig) -> Result<(), Error> {
        check_addresses(
            chain,
            workers(&path.executor, [&path.send_uln, &path.receive_uln]),
        )
    }

    fn rejected(result: Result<(), Error>, expected_role: &str, expected: &str) {
        assert!(
            matches!(
                &result,
                Err(Error::NotAWorkerConfig { role, reason, .. })
                    if *role == expected_role && *reason == expected
            ),
            "{result:?}"
        );
    }

    #[test]
    fn worker_config_pdas_are_accepted() {
        check(&RecordingChain::default(), &fixture_path()).unwrap();
    }

    #[test]
    fn an_executor_program_id_is_rejected() {
        // What the mainnet path held: the worker's program, not its config account.
        let path = path_with(worker_program(), DVN.parse().unwrap());

        rejected(
            check(&RecordingChain::default(), &path),
            EXECUTOR_ROLE,
            "is a program",
        );
    }

    /// Each role has its own seed, so a DVN config named as the executor (or the reverse)
    /// fails every quote just like a program ID does.
    #[test]
    fn swapped_roles_are_rejected() {
        let chain = RecordingChain::default();

        rejected(
            check(
                &chain,
                &path_with(DVN.parse().unwrap(), DVN.parse().unwrap()),
            ),
            EXECUTOR_ROLE,
            "is not its owner's config PDA for that role",
        );
        rejected(
            check(
                &chain,
                &path_with(EXECUTOR.parse().unwrap(), EXECUTOR.parse().unwrap()),
            ),
            DVN_ROLE,
            "is not its owner's config PDA for that role",
        );
    }

    #[test]
    fn a_missing_wallet_or_unowned_dvn_is_rejected() {
        let mut chain = RecordingChain::default();
        let wallet = Pubkey::new_unique();
        chain.accounts.insert(wallet, Account::default());
        // The right PDA, but under an account that is not a program.
        let data_account = Pubkey::new_unique();
        let under_data = Pubkey::find_program_address(&[DVN_CONFIG_SEED], &data_account).0;
        chain.accounts.insert(data_account, Account::default());
        chain.accounts.insert(
            under_data,
            Account {
                owner: data_account,
                ..Account::default()
            },
        );

        [
            (Pubkey::new_unique(), "does not exist"),
            (wallet, "is not its owner's config PDA for that role"),
            (under_data, "is not owned by a program"),
        ]
        .into_iter()
        .for_each(|(dvn, reason)| {
            let path = path_with(EXECUTOR.parse().unwrap(), dvn);

            rejected(check(&chain, &path), DVN_ROLE, reason);
        });
    }

    /// The same DVN repeats across both ULN configs and every peer; each distinct worker is read
    /// once (and its owner once).
    #[test]
    fn each_distinct_worker_is_checked_once() {
        let path = fixture_path();
        let workers: BTreeSet<_> = (0..3)
            .flat_map(|_| workers(&path.executor, [&path.send_uln, &path.receive_uln]))
            .collect();

        assert_eq!(
            workers.into_iter().collect::<Vec<_>>(),
            [
                (DVN_ROLE, DVN.parse().unwrap()),
                (EXECUTOR_ROLE, EXECUTOR.parse().unwrap())
            ]
        );
    }

    /// A configured path is whatever the chain holds and, once the program is final, can never
    /// change: checking its workers would only block every later plan.
    #[test]
    fn check_workers_skips_paths_already_configured() {
        let mut plan = plan(5_000_000);
        let mut chain = RecordingChain::default();
        chain.accounts.remove(&DVN.parse().unwrap());

        rejected(check_workers(&chain, &plan), DVN_ROLE, "does not exist");
        let peer = &plan.inputs.layerzero_peers[0];
        plan.live_configs.layerzero_paths = vec![Path::expected(
            &release_address(LAYERZERO_PROVER),
            &peer.into(),
            &peer.path,
        )];
        check_workers(&chain, &plan).unwrap();
    }

    /// Finalize checks what it is about to lock in: the live paths.
    #[test]
    fn check_live_workers_reads_the_paths_on_chain() {
        let plan = plan(5_000_000);
        let peer = &plan.inputs.layerzero_peers[0];
        let program = release_address(LAYERZERO_PROVER);
        let live = Path::expected(&program, &peer.into(), &peer.path);
        let wrong = Path::expected(
            &program,
            &peer.into(),
            &path_with(worker_program(), DVN.parse().unwrap()),
        );
        let chain = RecordingChain::default();

        check_live_workers(&chain, &[live]).unwrap();
        rejected(
            check_live_workers(&chain, &[wrong]),
            EXECUTOR_ROLE,
            "is a program",
        );
    }
}
