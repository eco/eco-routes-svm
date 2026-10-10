pub mod common;

use std::collections::BTreeMap;

use anchor_lang::{system_program, AccountDeserialize, InstructionData, ToAccountMetas};
use common::litesvm_chain::{uln_account_data, LitesvmChain};
use common::{Context, RELEASE_PROGRAMS};
use deployer::apply::{self, apply, Step};
use deployer::classify::{ProgramState, Status};
use deployer::config::{self, Configs};
use deployer::plan::{
    self, Cluster, Plan, Release, ReleaseProgram, AGGREGATOR_MEMBERS, AGGREGATOR_PROVER,
    HYPER_PROVER, LAYERZERO_PROVER, LOCAL_PROVER, POLYMER_PROVER,
};
use deployer::readback::{self, readback};
use deployer::{setup, Chain, Inputs, RawInputs};
use layerzero_prover::instructions::{required_alt_addresses, InitArgs};
use layerzero_prover::layerzero;
use layerzero_prover::state::{LzReceiveTypesAccount, Peer, Store};
use solana_sdk::account::Account;
use solana_sdk::clock::Clock;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::sysvar::slot_hashes::SlotHashes;

const HYPER_SENDER: &str = "0xAbCdEf0123456789aBcDeF0123456789abcdef01";
const OTHER_SENDER: &str = "0x1111111111111111111111111111111111111111";
const POLYMER_EMITTER: &str = "0x2222222222222222222222222222222222222222";
const DVN: &str = "11111111111111111111111111111112";
const OTHER_DVN: &str = "11111111111111111111111111111114";
const EXECUTOR: &str = "11111111111111111111111111111113";
const HYPER_RESERVE: u64 = 1_000_000;
const LAYERZERO_RESERVE: u64 = 1_000_000_000;
const SLOT_HASHES_SPAN: u64 = 512;
const PEER_CHAIN_ID: u64 = 30184;
const LOOKUP_TABLE_META_SIZE: usize = 56;
const LOOKUP_TABLE_AUTHORITY_OFFSET: usize = 21;
const TRANSFER_LAMPORTS: u64 = 1_000_000;

type ErrorCheck = fn(&readback::Error) -> bool;

struct Env {
    context: Context,
    deployer: Keypair,
    release: Release,
}

impl Env {
    fn new() -> Self {
        let mut context = Context::default();
        let deployer = Keypair::new();
        context.airdrop(&deployer.pubkey(), 10_000_000_000).unwrap();
        RELEASE_PROGRAMS
            .iter()
            .for_each(|program| context.set_upgrade_authority(program, Some(deployer.pubkey())));
        // The lookup-table program only accepts a `recent_slot` listed in `SlotHashes`, which
        // litesvm leaves empty and does not advance; list the slots the test transactions reach.
        let slot = context.get_sysvar::<Clock>().slot;
        let slot_hashes: Vec<_> = (slot..slot + SLOT_HASHES_SPAN)
            .rev()
            .map(|slot| (slot, Hash::default()))
            .collect();
        context.set_sysvar(&SlotHashes::new(&slot_hashes));

        Self {
            context,
            deployer,
            release: release([
                (HYPER_PROVER, hyper_prover::ID),
                (LOCAL_PROVER, local_prover::ID),
                (POLYMER_PROVER, polymer_prover::ID),
                (AGGREGATOR_PROVER, aggregator_prover::ID),
                (LAYERZERO_PROVER, layerzero_prover::ID),
            ]),
        }
    }

    fn chain(&mut self) -> LitesvmChain<'_> {
        LitesvmChain::new(&mut self.context)
    }

    fn plan(&mut self, inputs: RawInputs) -> Result<Plan, plan::Error> {
        let release = self.release.clone();
        let states = states(&release, &self.deployer.pubkey());
        let live_configs = config::read(&self.chain(), &release).unwrap();
        let genesis_hash = self.chain().genesis_hash().unwrap();

        Plan::build(
            release,
            genesis_hash,
            self.deployer.pubkey(),
            states,
            live_configs,
            Inputs::parse(inputs).unwrap(),
        )
    }

    /// At the plan's compute-unit price, as `deployer apply` runs it.
    fn apply(&mut self, plan: &Plan) -> Result<Vec<Step>, apply::Error> {
        let deployer = self.deployer.insecure_clone();
        let mut chain = self
            .chain()
            .with_compute_unit_price(plan.inputs.compute_unit_price);
        let mut steps = Vec::new();
        apply(&mut chain, plan, &deployer, &mut |step| {
            steps.push(step.clone())
        })?;

        Ok(steps)
    }

    fn balance(&self, address: &Pubkey) -> u64 {
        self.context
            .get_account(address)
            .map_or(0, |account| account.lamports)
    }
}

fn reserve_address(program: &Pubkey, seed: &[u8]) -> Pubkey {
    Pubkey::find_program_address(&[seed], program).0
}

fn hyper_reserve() -> Pubkey {
    reserve_address(&hyper_prover::ID, hyper_prover::state::PDA_PAYER_SEED)
}

fn layerzero_reserve() -> Pubkey {
    reserve_address(
        &layerzero_prover::ID,
        layerzero_prover::state::PDA_PAYER_SEED,
    )
}

fn release<const N: usize>(programs: [(&str, Pubkey); N]) -> Release {
    Release {
        version: "0.0.1".into(),
        cluster: Cluster::Devnet,
        programs: programs
            .into_iter()
            .map(|(name, address)| {
                (
                    name.to_owned(),
                    ReleaseProgram {
                        address,
                        so_sha256: [0; 32],
                        dependencies: match name {
                            AGGREGATOR_PROVER => AGGREGATOR_MEMBERS.map(Into::into).to_vec(),
                            _ => vec![],
                        },
                    },
                )
            })
            .collect(),
    }
}

fn states(release: &Release, deployer: &Pubkey) -> BTreeMap<String, ProgramState> {
    release
        .programs
        .iter()
        .map(|(name, program)| {
            let state = ProgramState {
                address: program.address,
                status: Status::Partial,
                authority: Some(*deployer),
                data_hash: None,
            };

            (name.clone(), state)
        })
        .collect()
}

fn uln_json() -> String {
    format!(
        r#"{{"confirmations":15,"required_dvn_count":1,"optional_dvn_count":255,"optional_dvn_threshold":0,"required_dvns":["{DVN}"],"optional_dvns":[]}}"#
    )
}

fn inputs(hyper_senders: &str) -> RawInputs {
    inputs_with_peer_chain_id(hyper_senders, PEER_CHAIN_ID)
}

fn inputs_with_peer_chain_id(hyper_senders: &str, chain_id: u64) -> RawInputs {
    let uln = uln_json();

    RawInputs {
        hyper_senders: hyper_senders.into(),
        polymer_emitters: POLYMER_EMITTER.into(),
        layerzero: format!(
            r#"{{"peers":[{{"eid":30184,"address":"{OTHER_SENDER}","chain_id":{chain_id},"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}]}}"#
        ),
        hyper_reserve_lamports: HYPER_RESERVE.to_string(),
        layerzero_reserve_lamports: LAYERZERO_RESERVE.to_string(),
        compute_unit_price: "0".into(),
    }
}

/// The reserve tops up to its target on every run, and LayerZero paths spend it, so a re-run
/// refills it; every other step must be a no-op.
fn without_reserve_top_ups(report: &[Step]) -> impl Iterator<Item = &Step> {
    report.iter().filter(|step| step.action != "fund_reserve")
}

fn sent(report: &[Step]) -> Vec<(&'static str, &'static str, bool)> {
    report
        .iter()
        .map(|step| (step.program, step.action, step.signature.is_some()))
        .collect()
}

#[test]
fn apply_initializes_hyper_polymer_and_aggregator() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();

    let report = env.apply(&plan).unwrap();

    assert_eq!(
        sent(&report),
        vec![
            (HYPER_PROVER, "fund_reserve", true),
            (LAYERZERO_PROVER, "fund_reserve", true),
            (HYPER_PROVER, "init", true),
            (POLYMER_PROVER, "init", true),
            (AGGREGATOR_PROVER, "init", true),
            (LAYERZERO_PROVER, "init", true),
            (LAYERZERO_PROVER, "init_path", true),
            (LAYERZERO_PROVER, "set_path_config", true),
            (LAYERZERO_PROVER, "set_alt", true),
        ]
    );
    assert_eq!(env.balance(&hyper_reserve()), HYPER_RESERVE);
    assert!(env.balance(&layerzero_reserve()) <= LAYERZERO_RESERVE);
    let live = config::read(&env.chain(), &plan.release).unwrap();
    live.deviation(&plan.expected_configs).unwrap();
    readback(&env.chain(), &plan).unwrap();
}

#[test]
fn apply_pays_the_priority_fee_for_the_whole_compute_unit_limit() {
    let spent = |compute_unit_price: &str| {
        let mut env = Env::new();
        let plan = env
            .plan(RawInputs {
                compute_unit_price: compute_unit_price.into(),
                ..inputs(HYPER_SENDER)
            })
            .unwrap();
        let before = env.balance(&env.deployer.pubkey());
        env.apply(&plan).unwrap();

        before - env.balance(&env.deployer.pubkey())
    };
    // Two reserve transfers, three prover inits, LayerZero `init`, `init_path`,
    // `set_path_config` and two lookup-table transactions.
    let transactions = 10;

    let unpriced = spent("0");
    let priced = spent("3");

    // 3 micro-lamports for 1.4M units is 4.2 lamports, rounded up.
    assert_eq!(priced - unpriced, transactions * 5);
}

#[test]
fn a_priced_transaction_costs_the_signature_fee_and_the_priority_fee() {
    let mut env = Env::new();
    let payer = env.deployer.insecure_clone();
    let transfer = solana_system_interface::instruction::transfer(
        &payer.pubkey(),
        &Pubkey::new_unique(),
        TRANSFER_LAMPORTS,
    );
    let before = env.balance(&payer.pubkey());

    env.chain()
        .with_compute_unit_price(3)
        .send(&[transfer], &[&payer])
        .unwrap();

    assert_eq!(
        before - env.balance(&payer.pubkey()),
        TRANSFER_LAMPORTS + 5_000 + 5
    );
}

#[test]
fn apply_rerun_sends_nothing() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    env.apply(&plan).unwrap();

    let report = env.apply(&plan).unwrap();

    assert!(without_reserve_top_ups(&report).all(|step| step.signature.is_none()));
    assert_eq!(report.len(), 9);
}

#[test]
fn apply_refuses_aggregator_before_members_deployed() {
    let mut env = Env::new();
    env.context
        .set_account(layerzero_prover::ID, Account::default())
        .unwrap();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();

    let result = env.apply(&plan);

    assert!(matches!(
        result,
        Err(apply::Error::MembersNotDeployed { members }) if members == [LAYERZERO_PROVER]
    ));
    let live = config::read(&env.chain(), &plan.release).unwrap();
    assert_eq!(live.aggregator_provers, None);
}

#[test]
fn apply_fails_on_existing_hyper_config_mismatch_without_sending_any_transaction() {
    let mut env = Env::new();
    let stale_plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    let unfunded = RawInputs {
        hyper_reserve_lamports: "0".into(),
        ..inputs(OTHER_SENDER)
    };
    let other_plan = env.plan(unfunded).unwrap();
    env.apply(&other_plan).unwrap();
    let before = config::read(&env.chain(), &stale_plan.release).unwrap();
    let deployer_balance = env.balance(&env.deployer.pubkey());
    let layerzero_reserve_balance = env.balance(&layerzero_reserve());

    let result = env.apply(&stale_plan);

    assert!(matches!(
        result,
        Err(apply::Error::Plan(plan::Error::ConfigMismatch(mismatch))) if mismatch.program == HYPER_PROVER
    ));
    assert_eq!(
        config::read(&env.chain(), &stale_plan.release).unwrap(),
        before
    );
    assert_eq!(env.balance(&hyper_reserve()), 0);
    assert_eq!(env.balance(&layerzero_reserve()), layerzero_reserve_balance);
    assert_eq!(env.balance(&env.deployer.pubkey()), deployer_balance);
}

#[test]
fn apply_tops_up_reserves_without_reducing() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    let (hyper_reserve, layerzero_reserve) = (hyper_reserve(), layerzero_reserve());
    let oversupplied = HYPER_RESERVE * 5;
    env.context.airdrop(&hyper_reserve, oversupplied).unwrap();
    env.context
        .airdrop(&layerzero_reserve, LAYERZERO_RESERVE / 2)
        .unwrap();

    let report = env.apply(&plan).unwrap();

    assert_eq!(
        sent(&report)[..2],
        [
            (HYPER_PROVER, "fund_reserve", false),
            (LAYERZERO_PROVER, "fund_reserve", true),
        ]
    );
    assert_eq!(env.balance(&hyper_reserve), oversupplied);
    let spent_on_paths = LAYERZERO_RESERVE - env.balance(&layerzero_reserve);
    assert!(spent_on_paths > 0 && spent_on_paths < LAYERZERO_RESERVE / 2);
}

#[test]
fn plan_refuses_an_rpc_of_another_cluster() {
    let mut env = Env::new();
    env.context.genesis_hash = Cluster::Mainnet.genesis_hash();

    let error = env.plan(inputs(HYPER_SENDER)).unwrap_err();

    assert!(
        matches!(
            error,
            plan::Error::ClusterMismatch {
                cluster: Cluster::Devnet,
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn readback_detects_mismatch() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    let absent = readback(&env.chain(), &plan);
    env.apply(&plan).unwrap();
    let stale = Plan::build(
        plan.release.clone(),
        plan.genesis_hash,
        plan.deployer,
        states(&plan.release, &env.deployer.pubkey()),
        Configs::default(),
        Inputs::parse(inputs(OTHER_SENDER)).unwrap(),
    )
    .unwrap();

    let result = readback(&env.chain(), &stale);

    assert!(matches!(
        absent,
        Err(readback::Error::Mismatch { program, actual, .. }) if program == HYPER_PROVER && actual == "absent"
    ));
    assert!(matches!(
        result,
        Err(readback::Error::Mismatch { program, .. }) if program == HYPER_PROVER
    ));
}

#[test]
fn plan_fails_when_live_config_differs_from_inputs() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    env.apply(&plan).unwrap();

    let result = env.plan(inputs(OTHER_SENDER));

    assert!(matches!(
        result,
        Err(plan::Error::ConfigMismatch(mismatch)) if mismatch.program == HYPER_PROVER
    ));
    assert!(env.plan(inputs(HYPER_SENDER)).is_ok());
}

fn store_address() -> Pubkey {
    Pubkey::find_program_address(
        &[layerzero_prover::state::STORE_SEED],
        &layerzero_prover::ID,
    )
    .0
}

fn store(env: &Env) -> Store {
    let account = env.context.get_account(&store_address()).unwrap();

    Store::try_deserialize(&mut account.data.as_slice()).unwrap()
}

fn alt_account(env: &Env) -> Account {
    env.context.get_account(&store(env).alt).unwrap()
}

fn alt_addresses(account: &Account) -> Vec<Pubkey> {
    account.data[LOOKUP_TABLE_META_SIZE..]
        .chunks_exact(32)
        .map(|address| Pubkey::try_from(address).unwrap())
        .collect()
}

#[test]
fn apply_sets_up_layerzero_paths_and_frozen_alt() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();

    env.apply(&plan).unwrap();

    let store = store(&env);
    assert_eq!(store.peers.len(), 1);
    assert_eq!(store.peers[0].eid, 30184);
    assert_eq!(store.peers[0].chain_id, PEER_CHAIN_ID);
    assert_ne!(store.alt, Pubkey::default());
    let alt = alt_account(&env);
    assert_eq!(alt.data[LOOKUP_TABLE_AUTHORITY_OFFSET], 0);
    assert_eq!(alt_addresses(&alt), required_alt_addresses(&store));
    let live = config::read(&env.chain(), &plan.release).unwrap();
    assert_eq!(live.layerzero_alt.map(|alt| alt.address), Some(store.alt));
    readback(&env.chain(), &plan).unwrap();
}

#[test]
fn apply_layerzero_rerun_sends_nothing() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    env.apply(&plan).unwrap();
    let alt = store(&env).alt;

    let report = env.apply(&plan).unwrap();

    assert!(without_reserve_top_ups(&report).all(|step| step.signature.is_none()));
    assert_eq!(store(&env).alt, alt);
}

#[test]
fn apply_fails_on_existing_layerzero_peers_mismatch() {
    let mut env = Env::new();
    let stale_plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    let other_plan = env
        .plan(inputs_with_peer_chain_id(HYPER_SENDER, PEER_CHAIN_ID + 1))
        .unwrap();
    env.apply(&other_plan).unwrap();
    let before = config::read(&env.chain(), &stale_plan.release).unwrap();
    let deployer_balance = env.balance(&env.deployer.pubkey());
    let reserve_balance = env.balance(&layerzero_reserve());

    let result = env.apply(&stale_plan);

    assert!(matches!(
        result,
        Err(apply::Error::Plan(plan::Error::ConfigMismatch(mismatch))) if mismatch.program == LAYERZERO_PROVER
    ));
    assert_eq!(
        config::read(&env.chain(), &stale_plan.release).unwrap(),
        before
    );
    assert_eq!(env.balance(&env.deployer.pubkey()), deployer_balance);
    assert_eq!(env.balance(&layerzero_reserve()), reserve_balance);
    assert!(matches!(
        env.plan(inputs(HYPER_SENDER)),
        Err(plan::Error::ConfigMismatch(mismatch)) if mismatch.program == LAYERZERO_PROVER
    ));
}

fn path_accounts(env: &Env) -> deployer::layerzero_state::PathAccounts {
    let peer = store(env).peers[0];

    deployer::layerzero_state::path_accounts(&layerzero_prover::ID, &peer)
}

fn applied_env() -> (Env, Plan) {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    env.apply(&plan).unwrap();

    (env, plan)
}

#[test]
fn readback_detects_a_bad_alt() {
    let (mut env, plan) = applied_env();
    let alt_address = store(&env).alt;
    let pristine = alt_account(&env);
    let changed = |change: fn(&mut Account)| {
        let mut account = pristine.clone();
        change(&mut account);

        account
    };
    let cases: [(&str, Account, ErrorCheck); 4] = [
        (
            "unfrozen",
            changed(|account| account.data[LOOKUP_TABLE_AUTHORITY_OFFSET] = 1),
            |error| {
                matches!(
                    error,
                    readback::Error::Incomplete(setup::Incomplete::AltUnfrozen { .. })
                )
            },
        ),
        (
            "deactivated",
            changed(|account| account.data[4..12].copy_from_slice(&5u64.to_le_bytes())),
            |error| {
                matches!(
                    error,
                    readback::Error::Incomplete(setup::Incomplete::AltDeactivated { .. })
                )
            },
        ),
        (
            "incomplete",
            changed(|account| account.data.truncate(account.data.len() - 32)),
            |error| {
                matches!(
                    error,
                    readback::Error::Incomplete(setup::Incomplete::AltIncomplete { .. })
                )
            },
        ),
        (
            "not a lookup table",
            changed(|account| account.owner = Pubkey::new_unique()),
            |error| {
                matches!(
                    error,
                    readback::Error::Incomplete(setup::Incomplete::AltInvalid { .. })
                )
            },
        ),
    ];

    cases.into_iter().for_each(|(name, account, expected)| {
        env.context.set_account(alt_address, account).unwrap();

        let error = readback(&env.chain(), &plan).unwrap_err();

        assert!(expected(&error), "{name}: {error:?}");
    });
    env.context.set_account(alt_address, pristine).unwrap();
    readback(&env.chain(), &plan).unwrap();
}

#[test]
fn readback_detects_an_unrecorded_alt() {
    let (mut env, plan) = applied_env();
    let store_account = env.context.get_account(&store_address()).unwrap();
    let mut unrecorded = store_account.clone();
    let alt = store(&env).alt;
    let alt_offset = unrecorded
        .data
        .windows(32)
        .position(|window| window == alt.as_ref())
        .unwrap();
    unrecorded.data[alt_offset..alt_offset + 32].fill(0);
    env.context
        .set_account(store_address(), unrecorded)
        .unwrap();

    let result = readback(&env.chain(), &plan);

    assert!(matches!(
        result,
        Err(readback::Error::Incomplete(setup::Incomplete::AltNotSet))
    ));
}

#[test]
fn readback_detects_each_missing_path_account() {
    let (mut env, plan) = applied_env();
    let accounts = path_accounts(&env);

    [
        accounts.nonce,
        accounts.send_config,
        accounts.receive_config,
    ]
    .into_iter()
    .for_each(|address| {
        let account = env.context.get_account(&address).unwrap();
        env.context
            .set_account(address, Account::default())
            .unwrap();

        let result = readback(&env.chain(), &plan);

        assert!(
            matches!(
                result,
                Err(readback::Error::Incomplete(
                    setup::Incomplete::PathMissing { account, .. }
                )) if account == address
            ),
            "{address}: {result:?}"
        );
        env.context.set_account(address, account).unwrap();
    });
    readback(&env.chain(), &plan).unwrap();
}

#[test]
fn readback_detects_a_wrong_uln_config() {
    let (mut env, plan) = applied_env();
    let accounts = path_accounts(&env);
    let send = env.context.get_account(&accounts.send_config).unwrap();
    let live = config::read(&env.chain(), &plan.release).unwrap();
    let mut executor = live.layerzero_paths[0].send.clone().unwrap().executor;
    executor.max_message_size += 1;
    let uln = live.layerzero_paths[0].send.clone().unwrap().uln;
    let wrong = Account {
        data: uln_account_data("SendConfig", &(255u8, uln, executor)),
        ..send
    };
    env.context
        .set_account(accounts.send_config, wrong)
        .unwrap();

    let result = readback(&env.chain(), &plan);

    assert!(matches!(
        result,
        Err(readback::Error::Mismatch { program, .. }) if program == LAYERZERO_PROVER
    ));
}

#[test]
fn plan_and_apply_refuse_an_existing_path_config_with_other_dvns() {
    let mut env = Env::new();
    let stale_plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    let other_plan = env.plan(inputs_with_dvn(OTHER_DVN)).unwrap();
    env.apply(&other_plan).unwrap();
    let deployer_balance = env.balance(&env.deployer.pubkey());

    let planned = env.plan(inputs(HYPER_SENDER));
    let applied = env.apply(&stale_plan);

    assert!(matches!(
        planned,
        Err(plan::Error::ConfigMismatch(mismatch)) if mismatch.program == LAYERZERO_PROVER
    ));
    assert!(matches!(
        applied,
        Err(apply::Error::Plan(plan::Error::ConfigMismatch(mismatch))) if mismatch.program == LAYERZERO_PROVER
    ));
    assert_eq!(env.balance(&env.deployer.pubkey()), deployer_balance);
}

#[test]
fn unreadable_uln_config_fails_closed() {
    let (mut env, plan) = applied_env();
    let accounts = path_accounts(&env);
    let send = env.context.get_account(&accounts.send_config).unwrap();
    let corrupt = |change: fn(&mut Account)| {
        let mut account = send.clone();
        change(&mut account);

        account
    };

    [
        corrupt(|account| account.owner = Pubkey::new_unique()),
        corrupt(|account| account.data[..8].fill(0)),
        corrupt(|account| account.data.truncate(12)),
    ]
    .into_iter()
    .for_each(|account| {
        env.context
            .set_account(accounts.send_config, account)
            .unwrap();

        assert!(config::read(&env.chain(), &plan.release).is_err());
        assert!(readback(&env.chain(), &plan).is_err());
        assert!(env.apply(&plan).is_err());
    });
}

fn inputs_with_dvn(dvn: &str) -> RawInputs {
    let raw = inputs(HYPER_SENDER);

    RawInputs {
        layerzero: raw.layerzero.replace(DVN, dvn),
        ..raw
    }
}

fn inputs_with_peer_count(count: u32) -> RawInputs {
    let uln = uln_json();
    let peers = (1..=count)
        .map(|index| {
            format!(
                r#"{{"eid":{},"address":"0x{:040x}","chain_id":{},"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}"#,
                30000 + index,
                index,
                1000 + index
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    RawInputs {
        layerzero: format!(r#"{{"peers":[{peers}]}}"#),
        ..inputs(HYPER_SENDER)
    }
}

#[test]
fn apply_fills_alt_in_chunks_for_the_maximum_peer_set() {
    let mut env = Env::new();
    let max_peers = layerzero_prover::state::MAX_PEERS as u32;
    let plan = env.plan(inputs_with_peer_count(max_peers)).unwrap();

    env.apply(&plan).unwrap();

    let store = store(&env);
    let required = required_alt_addresses(&store);
    assert!(required.len() > 20);
    assert_eq!(alt_addresses(&alt_account(&env)), required);
    readback(&env.chain(), &plan).unwrap();
}

/// `init` sent outside `apply`, with `peers`.
fn init_layerzero_store(env: &mut Env, peers: Vec<Peer>) {
    let store = store_address();
    let deployer = env.deployer.insecure_clone();
    let instruction = Instruction {
        program_id: layerzero_prover::ID,
        accounts: layerzero_prover::accounts::Init {
            payer: deployer.pubkey(),
            authority: deployer.pubkey(),
            program: layerzero_prover::ID,
            program_data: config::program_data_address(&layerzero_prover::ID),
            store,
            lz_receive_types: LzReceiveTypesAccount::pda().0,
            system_program: system_program::ID,
            endpoint_program: layerzero::ENDPOINT_ID,
            oapp_registry: layerzero::oapp_registry_pda(&store).0,
            endpoint_event_authority: layerzero::endpoint_event_authority().0,
        }
        .to_account_metas(None),
        data: layerzero_prover::instruction::Init {
            args: InitArgs { peers },
        }
        .data(),
    };

    env.chain().send(&[instruction], &[&deployer]).unwrap();
}

#[test]
fn apply_resumes_from_an_initialized_store() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    let peers = plan.expected_configs.layerzero_peers.clone().unwrap();
    init_layerzero_store(&mut env, peers);

    let report = env.apply(&plan).unwrap();

    assert_eq!(
        sent(&report)[5..],
        [
            (LAYERZERO_PROVER, "init", false),
            (LAYERZERO_PROVER, "init_path", true),
            (LAYERZERO_PROVER, "set_path_config", true),
            (LAYERZERO_PROVER, "set_alt", true),
        ]
    );
    readback(&env.chain(), &plan).unwrap();
}

#[test]
fn apply_initializes_every_peer_in_one_init_transaction() {
    let mut env = Env::new();
    let max_peers = layerzero_prover::state::MAX_PEERS as u32;
    let plan = env.plan(inputs_with_peer_count(max_peers)).unwrap();
    let peers = plan.expected_configs.layerzero_peers.clone().unwrap();

    let report = env.apply(&plan).unwrap();

    assert_eq!(
        sent(&report)[5..7],
        [
            (LAYERZERO_PROVER, "init", true),
            (LAYERZERO_PROVER, "init_path", true),
        ]
    );
    assert_eq!(store(&env).peers, peers);
    readback(&env.chain(), &plan).unwrap();
}

#[test]
fn apply_refuses_a_store_holding_only_the_first_planned_peers() {
    let mut env = Env::new();
    let plan = env.plan(inputs_with_peer_count(20)).unwrap();
    let peers = plan.expected_configs.layerzero_peers.clone().unwrap();
    init_layerzero_store(&mut env, peers[..16].to_vec());

    let error = env.apply(&plan).unwrap_err();

    assert!(
        matches!(
            &error,
            apply::Error::Plan(deployer::plan::Error::ConfigMismatch(mismatch))
                if mismatch.program == LAYERZERO_PROVER
        ),
        "{error:?}"
    );
    assert_eq!(store(&env).peers, peers[..16]);
}

#[test]
fn apply_initializes_a_path_whose_nonce_address_was_prefunded() {
    let mut env = Env::new();
    let plan = env.plan(inputs(HYPER_SENDER)).unwrap();
    let peer = plan.expected_configs.layerzero_peers.clone().unwrap()[0];
    let nonce = layerzero_prover::layerzero::nonce_pda(&store_address(), peer.eid, &peer.address).0;
    env.context.airdrop(&nonce, 1_000_000).unwrap();

    let report = env.apply(&plan).unwrap();

    assert!(sent(&report).contains(&(LAYERZERO_PROVER, "init_path", true)));
    readback(&env.chain(), &plan).unwrap();
}
