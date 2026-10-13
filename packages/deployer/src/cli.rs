use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use solana_sdk::pubkey::Pubkey;

use crate::plan::{self, Cluster};

#[derive(Debug, Parser)]
#[command(
    name = "deployer",
    about = "Plans and applies an Eco Routes SVM release"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Classify the release against the chain, write `plan.json` and print `plan_hash=<hex>`.
    Plan(Box<PlanArgs>),
    /// Rebuild the plan from the chain, refuse unless its hash is the one in `plan.json`, then
    /// run the init steps and read them back.
    Apply(ApplyArgs),
    /// Plan making the selected upgradeable programs immutable; writes `plan.json` and prints
    /// `plan_hash=<hex>`. Refuses a program before its dependencies, or one not initialized or
    /// verified.
    #[command(name = "plan-finalize")]
    PlanFinalize(SelectionArgs),
    /// Plan closing the selected upgradeable programs, which burns their addresses; writes
    /// `plan.json` and prints `plan_hash=<hex>`.
    #[command(name = "plan-close")]
    PlanClose(SelectionArgs),
    /// Print the programs `plan.json` assigns to an action, one per line, each after the
    /// programs it depends on (before them for `close`).
    Actions(ActionsArgs),
}

/// The RPC and the deployer's address, for the plan commands: they only read the chain, so they
/// never need the deployer's key.
#[derive(Debug, Args)]
pub struct ReadArgs {
    /// RPC endpoint; it carries the API key, so it is never printed.
    #[arg(long, env = "RPC_URL", hide_env_values = true)]
    pub rpc_url: String,
    /// The deployer's address: the upgrade authority the plan expects on partial programs.
    #[arg(long, env = "DEPLOYER")]
    pub deployer: Pubkey,
}

#[derive(Debug, Args)]
pub struct ChainArgs {
    /// RPC endpoint; it carries the API key, so it is never printed.
    #[arg(long, env = "RPC_URL", hide_env_values = true)]
    pub rpc_url: String,
    /// JSON keypair file of the deployer; never printed.
    #[arg(long)]
    pub deployer_keypair: PathBuf,
}

#[derive(Debug, Args)]
pub struct InputArgs {
    #[arg(long, env = "HYPER_SENDERS")]
    pub hyper_senders: String,
    #[arg(long, env = "POLYMER_EMITTERS")]
    pub polymer_emitters: String,
    #[arg(long, env = "LAYERZERO")]
    pub layerzero: String,
    #[arg(long, env = "HYPER_RESERVE_LAMPORTS")]
    pub hyper_reserve_lamports: String,
    #[arg(long, env = "LAYERZERO_RESERVE_LAMPORTS")]
    pub layerzero_reserve_lamports: String,
    /// Micro-lamports per compute unit for the run's deploys and `apply`; part of the plan hash.
    #[arg(long, env = "COMPUTE_UNIT_PRICE", default_value = "0")]
    pub compute_unit_price: String,
}

#[derive(Debug, Args)]
pub struct PlanArgs {
    #[arg(long)]
    pub version: String,
    #[arg(long)]
    pub cluster: Cluster,
    /// `program-ids.json`: `{ "<program>": { "address", "seed", "salt" } }`.
    #[arg(long)]
    pub program_ids: PathBuf,
    /// Directory of the release `<program>.<cluster>.so` files.
    #[arg(long)]
    pub assets: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    /// File the plan summary is appended to (`$GITHUB_STEP_SUMMARY`).
    #[arg(long)]
    pub summary: Option<PathBuf>,
    #[command(flatten)]
    pub chain: ReadArgs,
    #[command(flatten)]
    pub inputs: InputArgs,
}

#[derive(Debug, Args)]
pub struct SelectionArgs {
    #[arg(long)]
    pub version: String,
    #[arg(long)]
    pub cluster: Cluster,
    /// `program-ids.json`: `{ "<program>": { "address", "seed", "salt", "dependencies" } }`.
    #[arg(long)]
    pub program_ids: PathBuf,
    /// Directory of the release `<program>.<cluster>.so` files.
    #[arg(long)]
    pub assets: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    /// File the plan summary is appended to (`$GITHUB_STEP_SUMMARY`).
    #[arg(long)]
    pub summary: Option<PathBuf>,
    /// `all`, or a comma-separated list of program names.
    #[arg(long, env = "PROGRAMS")]
    pub programs: String,
    #[command(flatten)]
    pub chain: ReadArgs,
}

#[derive(Debug, Args)]
pub struct ApplyArgs {
    #[arg(long)]
    pub plan: PathBuf,
    /// The release binaries the plan hashed.
    #[arg(long)]
    pub assets: PathBuf,
    #[command(flatten)]
    pub chain: ChainArgs,
}

#[derive(Debug, Args)]
pub struct ActionsArgs {
    #[arg(long)]
    pub plan: PathBuf,
    #[arg(long, value_enum)]
    pub action: ActionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ActionKind {
    Deploy,
    Verify,
    Finalize,
    Close,
}

impl From<ActionKind> for plan::Action {
    fn from(kind: ActionKind) -> Self {
        match kind {
            ActionKind::Deploy => Self::Deploy,
            ActionKind::Verify => Self::Verify,
            ActionKind::Finalize => Self::Finalize,
            ActionKind::Close => Self::Close,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAIN: [&str; 4] = [
        "--rpc-url",
        "http://rpc",
        "--deployer-keypair",
        "deployer.json",
    ];
    const READ: [&str; 4] = [
        "--rpc-url",
        "http://rpc",
        "--deployer",
        "11111111111111111111111111111112",
    ];
    const INPUTS: [&str; 10] = [
        "--hyper-senders",
        "0xaa",
        "--polymer-emitters",
        "0xbb",
        "--layerzero",
        "{}",
        "--hyper-reserve-lamports",
        "1",
        "--layerzero-reserve-lamports",
        "2",
    ];

    fn parse(arguments: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(["deployer"].into_iter().chain(arguments.iter().copied()))
    }

    #[test]
    fn plan_parses_every_argument() {
        let arguments = [
            &["plan", "--version", "1.2.3", "--cluster", "devnet"][..],
            &[
                "--program-ids",
                "ids.json",
                "--assets",
                "assets",
                "--out",
                "plan.json",
            ],
            &["--summary", "summary.md"],
            &READ,
            &INPUTS,
        ]
        .concat();

        let Command::Plan(plan) = parse(&arguments).unwrap().command else {
            panic!("expected the plan command");
        };

        assert_eq!(plan.version, "1.2.3");
        assert_eq!(plan.cluster, Cluster::Devnet);
        assert_eq!(plan.summary, Some("summary.md".into()));
        assert_eq!(plan.chain.rpc_url, "http://rpc");
        assert_eq!(
            plan.chain.deployer.to_string(),
            "11111111111111111111111111111112"
        );
        assert_eq!(plan.inputs.layerzero_reserve_lamports, "2");
    }

    #[test]
    fn plan_without_summary() {
        let arguments = [
            &["plan", "--version", "1", "--cluster", "mainnet"][..],
            &[
                "--program-ids",
                "ids.json",
                "--assets",
                "assets",
                "--out",
                "plan.json",
            ],
            &READ,
            &INPUTS,
        ]
        .concat();

        let Command::Plan(plan) = parse(&arguments).unwrap().command else {
            panic!("expected the plan command");
        };

        assert_eq!(plan.summary, None);
    }

    #[test]
    fn apply_parses_plan_assets_and_chain() {
        let arguments = [
            &["apply", "--plan", "plan.json", "--assets", "assets"][..],
            &CHAIN,
        ]
        .concat();

        let Command::Apply(apply) = parse(&arguments).unwrap().command else {
            panic!("expected the apply command");
        };

        assert_eq!(apply.plan, PathBuf::from("plan.json"));
        assert_eq!(apply.assets, PathBuf::from("assets"));
        assert_eq!(apply.chain.deployer_keypair, PathBuf::from("deployer.json"));
    }

    #[test]
    fn plan_takes_the_compute_unit_price_as_an_input() {
        let arguments = [
            &["plan", "--version", "1", "--cluster", "mainnet"][..],
            &[
                "--program-ids",
                "ids.json",
                "--assets",
                "assets",
                "--out",
                "plan.json",
            ],
            &READ,
            &INPUTS,
            &["--compute-unit-price", "5"],
        ]
        .concat();
        let without_price: Vec<&str> = arguments[..arguments.len() - 2].to_vec();

        let Command::Plan(plan) = parse(&arguments).unwrap().command else {
            panic!("expected the plan command");
        };
        let Command::Plan(default) = parse(&without_price).unwrap().command else {
            panic!("expected the plan command");
        };

        assert_eq!(plan.inputs.compute_unit_price, "5");
        assert_eq!(default.inputs.compute_unit_price, "0");
    }

    #[test]
    fn actions_parses_each_action_without_chain_arguments() {
        let parsed = ["deploy", "verify", "finalize", "close"].map(|name| {
            let arguments = ["actions", "--plan", "plan.json", "--action", name];
            let Command::Actions(actions) = parse(&arguments).unwrap().command else {
                panic!("expected the actions command");
            };

            actions.action
        });

        assert_eq!(
            parsed,
            [
                ActionKind::Deploy,
                ActionKind::Verify,
                ActionKind::Finalize,
                ActionKind::Close
            ]
        );
    }

    #[test]
    fn help_never_prints_the_rpc_url() {
        const SECRET: &str = "http://rpc.example/?api-key=help-must-not-print-this";
        std::env::set_var("RPC_URL", SECRET);

        ["plan", "apply"].iter().for_each(|command| {
            let help = parse(&[command, "--help"]).unwrap_err().to_string();

            assert!(help.contains("RPC_URL"), "{help}");
            assert!(!help.contains(SECRET), "{help}");
        });
    }

    #[test]
    fn malformed_arguments_are_rejected() {
        fn with_chain<'a>(arguments: &[&'a str]) -> Vec<&'a str> {
            [arguments, &CHAIN].concat()
        }

        let plan = |cluster: &'static str, deployer: &'static str| -> Vec<&'static str> {
            [
                &[
                    "plan",
                    "--version",
                    "1",
                    "--cluster",
                    cluster,
                    "--program-ids",
                    "i",
                    "--assets",
                    "a",
                    "--out",
                    "o",
                    "--rpc-url",
                    "http://rpc",
                    "--deployer",
                    deployer,
                ][..],
                &INPUTS,
            ]
            .concat()
        };
        let valid_deployer = READ[3];
        assert!(parse(&plan("devnet", valid_deployer)).is_ok());

        let rejected = [
            vec!["actions"],
            vec!["actions", "--plan", "p", "--action", "init"],
            with_chain(&["apply", "--plan", "p"]),
            plan("testnet", valid_deployer),
            plan("devnet", "not-an-address"),
        ];

        rejected
            .iter()
            .for_each(|arguments| assert!(parse(arguments).is_err(), "{arguments:?}"));
    }
}
