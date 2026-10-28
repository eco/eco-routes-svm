pub mod common;

use std::fs;
use std::path::Path;
use std::time::Duration;

use common::litesvm_chain::LitesvmChain;
use common::{Context, DVN, EXECUTOR, RELEASE_PROGRAMS};
use deployer::cli::{ActionKind, ActionsArgs, ApplyArgs, ChainArgs, InputArgs, PlanArgs, ReadArgs};
use deployer::commands::{self, Error};
use deployer::plan::{
    Cluster, PlanFile, PlanHash, AGGREGATOR_MEMBERS, AGGREGATOR_PROVER, CCTP_PROVER, HYPER_PROVER,
    LAYERZERO_PROVER, LOCAL_PROVER, POLYMER_PROVER,
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
const SLOT_HASHES_SPAN: u64 = 512;

fn programs() -> [(&'static str, Pubkey); 8] {
    [
        (AGGREGATOR_PROVER, aggregator_prover::ID),
        (CCTP_PROVER, cctp_prover::ID),
        (HYPER_PROVER, hyper_prover::ID),
        ("intent_chainer", intent_chainer::ID),
        (LAYERZERO_PROVER, layerzero_prover::ID),
        (LOCAL_PROVER, local_prover::ID),
        (POLYMER_PROVER, polymer_prover::ID),
        ("portal", portal::ID),
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

fn plan_args(directory: &Path, deployer: Pubkey) -> PlanArgs {
    let uln = format!(
        r#"{{"confirmations":15,"required_dvn_count":1,"optional_dvn_count":255,"optional_dvn_threshold":0,"required_dvns":["{DVN}"],"optional_dvns":[]}}"#
    );

    PlanArgs {
        programs: "all".into(),
        version: "0.0.1".into(),
        cluster: Cluster::Devnet,
        program_ids: directory.join("program-ids.json"),
        assets: directory.into(),
        out: directory.join("plan.json"),
        summary: Some(directory.join("summary.md")),
        chain: ReadArgs {
            rpc_url: "unused".into(),
            deployer,
        },
        inputs: InputArgs {
            hyper_senders: SENDER.into(),
            polymer_emitters: SENDER.into(),
            layerzero: format!(
                r#"{{"peers":[{{"eid":30184,"address":"{SENDER}","chain_id":30184,"path":{{"send_uln":{uln},"receive_uln":{uln},"executor":{{"max_message_size":10000,"executor":"{EXECUTOR}"}}}}}}]}}"#
            ),
            cctp: format!(r#"{{"peers":[{{"domain":0,"chain_id":1,"sender":"{SENDER}"}}]}}"#),
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
                AGGREGATOR_PROVER => serde_json::to_string(
                    &[AGGREGATOR_MEMBERS.as_slice(), &["portal"]].concat(),
                )
                .unwrap(),
                "portal" => "[]".to_owned(),
                _ => r#"["portal"]"#.to_owned(),
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
        .for_each(|((name, program), [(_, programdata), _])| {
            fs::write(
                directory.path().join(format!("{name}.devnet.so")),
                release_binary(programdata),
            )
            .unwrap();
            fs::write(
                directory.path().join(format!("{name}.devnet.json")),
                format!(r#"{{"address":"{program}","metadata":{{"name":"{name}"}}}}"#),
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
        &LitesvmChain::new(&mut context),
        &plan_args(directory.path(), deployer.pubkey()),
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
        &mut LitesvmChain::new(&mut context),
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
            "portal",
            CCTP_PROVER,
            HYPER_PROVER,
            "intent_chainer",
            LAYERZERO_PROVER,
            LOCAL_PROVER,
            POLYMER_PROVER,
            AGGREGATOR_PROVER
        ]
    );
    let printed = String::from_utf8(applied.clone()).unwrap();
    assert!(!printed.contains("intent_chainer init "), "{printed}");
    assert!(!printed.contains("portal init "), "{printed}");
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
        &mut LitesvmChain::new(&mut context),
        &apply_args(directory.path()),
        &deployer,
        &mut Vec::new(),
    );
    assert!(matches!(rerun, Err(Error::PlanChanged { expected, .. }) if expected == hash));
}

#[test]
fn selected_chainer_deploy_apply_and_idl_leave_inherited_programs_untouched() {
    let mut context = Context::default();
    context
        .add_program(
            deployer::idl::PROGRAM_METADATA_ID,
            include_bytes!("fixtures/program_metadata.so"),
        )
        .unwrap();
    let deployer = Keypair::new();
    context.airdrop(&deployer.pubkey(), 20_000_000_000).unwrap();
    context.set_upgrade_authority(&intent_chainer::ID, Some(deployer.pubkey()));
    context.set_upgrade_authority(&portal::ID, Some(Pubkey::new_unique()));
    context.set_upgrade_authority(&local_prover::ID, None);
    context.set_upgrade_authority(&hyper_prover::ID, Some(Pubkey::new_unique()));
    let chainer_accounts = deployed_accounts(&context, &intent_chainer::ID);
    let inherited: Vec<_> = programs()
        .into_iter()
        .filter(|(name, _)| *name != "intent_chainer")
        .flat_map(|(_, address)| deployed_accounts(&context, &address))
        .collect();
    let directory = TempDir::new().unwrap();
    let ids: serde_json::Map<_, _> = programs().into_iter().map(|(name, address)| {
        (name.into(), serde_json::json!({
            "address": address.to_string(),
            "dependencies": if name == "portal" { Vec::<&str>::new() } else { vec!["portal"] }
        }))
    }).collect();
    fs::write(
        directory.path().join("program-ids.json"),
        serde_json::to_vec(&ids).unwrap(),
    )
    .unwrap();
    fs::write(
        directory.path().join("deployment-policy.json"),
        serde_json::to_vec(&serde_json::json!({
            "nondeployable_programs": programs().into_iter().map(|(name, _)| name)
                .filter(|name| *name != "intent_chainer").collect::<Vec<_>>()
        }))
        .unwrap(),
    )
    .unwrap();
    [
        ("portal", portal::ID),
        ("intent_chainer", intent_chainer::ID),
    ]
    .into_iter()
    .for_each(|(name, address)| {
        let [(_, programdata), _] = deployed_accounts(&context, &address);
        fs::write(
            directory.path().join(format!("{name}.devnet.so")),
            release_binary(&programdata),
        )
        .unwrap();
    });
    fs::write(
        directory.path().join("intent_chainer.devnet.json"),
        format!(
            r#"{{"address":"{}","metadata":{{"name":"intent_chainer"}}}}"#,
            intent_chainer::ID
        ),
    )
    .unwrap();
    chainer_accounts.iter().for_each(|(address, _)| {
        context.set_account(*address, Account::default()).unwrap();
    });
    let args = PlanArgs {
        version: "3.1.0".into(),
        programs: "intent_chainer".into(),
        inputs: InputArgs {
            hyper_senders: String::new(),
            polymer_emitters: "unused".into(),
            layerzero: "unused".into(),
            cctp: String::new(),
            hyper_reserve_lamports: "unused".into(),
            layerzero_reserve_lamports: "unused".into(),
            compute_unit_price: "0".into(),
        },
        ..plan_args(directory.path(), deployer.pubkey())
    };
    let mut output = Vec::new();
    commands::plan(
        &LitesvmChain::new(&mut context),
        &args,
        &deployer.pubkey(),
        &mut output,
    )
    .unwrap();
    let hash = hash_line(&output);
    let file: PlanFile = serde_json::from_slice(&fs::read(&args.out).unwrap()).unwrap();
    assert_eq!(
        file.document
            .selected
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["intent_chainer"]
    );
    [ActionKind::Deploy, ActionKind::Verify]
        .into_iter()
        .for_each(|action| {
            let mut output = Vec::new();
            commands::actions(
                &ActionsArgs {
                    plan: args.out.clone(),
                    action,
                },
                &mut output,
            )
            .unwrap();
            assert_eq!(output, b"intent_chainer\n");
        });
    chainer_accounts.iter().for_each(|(address, account)| {
        context.set_account(*address, account.clone()).unwrap();
    });
    let mut output = Vec::new();
    commands::apply(
        &mut LitesvmChain::new(&mut context),
        &apply_args(directory.path()),
        &deployer,
        &mut output,
    )
    .unwrap();
    assert_eq!(hash_line(&output), hash);
    assert!(!String::from_utf8(output).unwrap().contains(" init "));
    let mut output = Vec::new();
    commands::idl(
        &mut LitesvmChain::new(&mut context),
        &file,
        directory.path(),
        &deployer,
        Duration::ZERO,
        &mut output,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(
        output
            .lines()
            .all(|line| line.starts_with("intent_chainer idl ")),
        "{output}"
    );
    assert!(context
        .get_account(&deployer::idl::canonical_address(&intent_chainer::ID))
        .is_some());
    assert!(context
        .get_account(&deployer::idl::canonical_address(&portal::ID))
        .is_none());
    inherited.into_iter().for_each(|(address, before)| {
        assert_eq!(context.get_account(&address).unwrap(), before, "{address}");
    });
}

#[test]
fn a_failing_command_prints_its_message_then_its_detail() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_deployer"))
        .args([
            "actions",
            "--plan",
            "missing-plan.json",
            "--action",
            "deploy",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(!output.status.success());
    assert!(
        stderr.starts_with("error: cannot read missing-plan.json: "),
        "{stderr}"
    );
    assert!(stderr.contains("Read {"), "{stderr}");
}
