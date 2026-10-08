pub mod common;

use std::fs;
use std::path::Path;

use common::litesvm_chain::LitesvmChain;
use common::{Context, RELEASE_PROGRAMS};
use deployer::cli::{ActionKind, ActionsArgs, ApplyArgs, ChainArgs, InputArgs, PlanArgs};
use deployer::commands::{self, Error};
use deployer::plan::{
    Cluster, PlanHash, AGGREGATOR_MEMBERS, AGGREGATOR_PROVER, HYPER_PROVER, LAYERZERO_PROVER,
    LOCAL_PROVER, POLYMER_PROVER,
};
use solana_loader_v3_interface::state::UpgradeableLoaderState;
use solana_sdk::account::Account;
use solana_sdk::clock::Clock;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::sysvar::slot_hashes::SlotHashes;
use tempfile::TempDir;

const SENDER: &str = "0xAbCdEf0123456789aBcDeF0123456789abcdef01";
const DVN: &str = "11111111111111111111111111111112";
const EXECUTOR: &str = "11111111111111111111111111111113";
const SLOT_HASHES_SPAN: u64 = 512;

fn programs() -> [(&'static str, Pubkey); 5] {
    [
        (AGGREGATOR_PROVER, aggregator_prover::ID),
        (HYPER_PROVER, hyper_prover::ID),
        (LAYERZERO_PROVER, layerzero_prover::ID),
        (LOCAL_PROVER, local_prover::ID),
        (POLYMER_PROVER, polymer_prover::ID),
    ]
}

/// The accounts a `solana program deploy` leaves behind for `program`.
fn deployed_accounts(context: &Context, program: &Pubkey) -> [(Pubkey, Account); 2] {
    let loader = context.get_account(program).unwrap().owner;
    let programdata = Pubkey::find_program_address(&[program.as_ref()], &loader).0;

    [programdata, *program].map(|address| (address, context.get_account(&address).unwrap()))
}

fn release_binary(programdata: &Account) -> Vec<u8> {
    programdata.data[UpgradeableLoaderState::size_of_programdata_metadata()..].to_vec()
}

fn chain_args() -> ChainArgs {
    ChainArgs {
        rpc_url: "unused".into(),
        deployer_keypair: "unused".into(),
    }
}

fn plan_args(directory: &Path) -> PlanArgs {
    let uln = format!(
        r#"{{"confirmations":15,"required_dvn_count":1,"optional_dvn_count":255,"optional_dvn_threshold":0,"required_dvns":["{DVN}"],"optional_dvns":[]}}"#
    );

    PlanArgs {
        version: "0.0.1".into(),
        cluster: Cluster::Devnet,
        program_ids: directory.join("program-ids.json"),
        assets: directory.into(),
        out: directory.join("plan.json"),
        summary: Some(directory.join("summary.md")),
        chain: chain_args(),
        inputs: InputArgs {
            hyper_senders: SENDER.into(),
            polymer_emitters: SENDER.into(),
            layerzero: format!(
                r#"{{"peers":[{{"eid":30184,"address":"{SENDER}","chain_id":30184,"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}]}}"#
            ),
            hyper_reserve_lamports: "1000000".into(),
            layerzero_reserve_lamports: "1000000000".into(),
            compute_unit_price: "0".into(),
        },
    }
}

fn apply_args(directory: &Path) -> ApplyArgs {
    ApplyArgs {
        plan: directory.join("plan.json"),
        assets: directory.into(),
        chain: chain_args(),
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

#[test]
fn plan_then_deploy_then_apply_agree_on_the_hash() {
    let mut context = Context::default();
    let deployer = Keypair::new();
    context.airdrop(&deployer.pubkey(), 20_000_000_000).unwrap();
    RELEASE_PROGRAMS
        .iter()
        .for_each(|program| context.set_upgrade_authority(program, Some(deployer.pubkey())));
    // The lookup-table program only takes a `recent_slot` listed in `SlotHashes`.
    let slot = context.get_sysvar::<Clock>().slot;
    let slot_hashes: Vec<_> = (slot..slot + SLOT_HASHES_SPAN)
        .rev()
        .map(|slot| (slot, Hash::default()))
        .collect();
    context.set_sysvar(&SlotHashes::new(&slot_hashes));

    let directory = TempDir::new().unwrap();
    let deployed: Vec<_> = programs()
        .iter()
        .map(|(_, program)| deployed_accounts(&context, program))
        .collect();
    let ids = programs()
        .iter()
        .map(|(name, program)| {
            let dependencies = match *name {
                AGGREGATOR_PROVER => serde_json::to_string(&AGGREGATOR_MEMBERS).unwrap(),
                _ => "[]".to_owned(),
            };

            format!(r#""{name}":{{"address":"{program}","seed":"","salt":0,"dependencies":{dependencies}}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        directory.path().join("program-ids.json"),
        format!("{{{ids}}}"),
    )
    .unwrap();
    programs()
        .iter()
        .zip(&deployed)
        .for_each(|((name, _), [(_, programdata), _])| {
            fs::write(
                directory.path().join(format!("{name}.devnet.so")),
                release_binary(programdata),
            )
            .unwrap();
        });
    // Before `solana program deploy`: nothing at the release addresses.
    deployed
        .iter()
        .flatten()
        .for_each(|(address, _)| context.set_account(*address, Account::default()).unwrap());
    assert!(programs()
        .iter()
        .all(|(_, program)| context.get_account(program).is_none()));

    let mut planned = Vec::new();
    commands::plan(
        &LitesvmChain(&mut context),
        &plan_args(directory.path()),
        &deployer.pubkey(),
        &mut planned,
    )
    .unwrap();
    let hash = hash_line(&planned);
    let mut to_deploy = Vec::new();
    let actions = ActionsArgs {
        plan: directory.path().join("plan.json"),
        action: ActionKind::Deploy,
    };
    commands::actions(&actions, &mut to_deploy).unwrap();

    deployed
        .iter()
        .flatten()
        .for_each(|(address, account)| context.set_account(*address, account.clone()).unwrap());
    let mut applied = Vec::new();
    commands::apply(
        &mut LitesvmChain(&mut context),
        &apply_args(directory.path()),
        &deployer,
        &mut applied,
    )
    .unwrap();

    assert_eq!(
        String::from_utf8(to_deploy)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            HYPER_PROVER,
            LAYERZERO_PROVER,
            LOCAL_PROVER,
            POLYMER_PROVER,
            AGGREGATOR_PROVER
        ]
    );
    let printed = String::from_utf8(applied.clone()).unwrap();
    assert!(printed.contains("hyper_prover init "), "{printed}");
    assert!(printed.contains("layerzero_prover set_alt "), "{printed}");
    assert!(
        printed.contains(&format!(
            "read back:\nhyper_prover: [0x{}{}]\n",
            "00".repeat(12),
            SENDER[2..].to_lowercase()
        )),
        "{printed}"
    );
    assert!(
        printed.contains("layerzero_prover path: eid 30184 nonce true send ["),
        "{printed}"
    );
    assert_eq!(hash_line(&applied), hash);

    // `apply` wrote the configs the hash covers, so the plan is spent: a rerun needs a new plan.
    let rerun = commands::apply(
        &mut LitesvmChain(&mut context),
        &apply_args(directory.path()),
        &deployer,
        &mut Vec::new(),
    );
    assert!(matches!(rerun, Err(Error::PlanChanged { expected, .. }) if expected == hash));
}
