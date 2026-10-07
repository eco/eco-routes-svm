use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

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
    /// Print the programs `plan.json` assigns to an action, one per line.
    Actions(ActionsArgs),
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
    #[arg(long, env = "FINALIZE_LAYERZERO", action = clap::ArgAction::Set, default_value = "false")]
    pub finalize_layerzero: bool,
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
    pub chain: ChainArgs,
    #[command(flatten)]
    pub inputs: InputArgs,
}

#[derive(Debug, Args)]
pub struct ApplyArgs {
    #[arg(long)]
    pub plan: PathBuf,
    /// The release binaries the plan hashed.
    #[arg(long)]
    pub assets: PathBuf,
    /// Micro-lamports per compute unit added to every transaction; 0 adds nothing.
    #[arg(long, default_value_t = 0)]
    pub compute_unit_price: u64,
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
}

impl From<ActionKind> for plan::Action {
    fn from(kind: ActionKind) -> Self {
        match kind {
            ActionKind::Deploy => Self::Deploy,
            ActionKind::Verify => Self::Verify,
            ActionKind::Finalize => Self::Finalize,
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
            &CHAIN,
            &INPUTS,
            &["--finalize-layerzero", "true"],
        ]
        .concat();

        let Command::Plan(plan) = parse(&arguments).unwrap().command else {
            panic!("expected the plan command");
        };

        assert_eq!(plan.version, "1.2.3");
        assert_eq!(plan.cluster, Cluster::Devnet);
        assert_eq!(plan.summary, Some("summary.md".into()));
        assert_eq!(plan.chain.rpc_url, "http://rpc");
        assert_eq!(plan.inputs.layerzero_reserve_lamports, "2");
        assert!(plan.inputs.finalize_layerzero);
    }

    #[test]
    fn plan_without_summary_does_not_finalize_by_default() {
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
            &CHAIN,
            &INPUTS,
        ]
        .concat();

        let Command::Plan(plan) = parse(&arguments).unwrap().command else {
            panic!("expected the plan command");
        };

        assert_eq!(plan.summary, None);
        assert!(!plan.inputs.finalize_layerzero);
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
        assert_eq!(apply.compute_unit_price, 0);
        assert_eq!(apply.chain.deployer_keypair, PathBuf::from("deployer.json"));
    }

    #[test]
    fn apply_parses_a_compute_unit_price() {
        let arguments = [
            &[
                "apply",
                "--plan",
                "p",
                "--assets",
                "a",
                "--compute-unit-price",
                "5",
            ][..],
            &CHAIN,
        ]
        .concat();

        let Command::Apply(apply) = parse(&arguments).unwrap().command else {
            panic!("expected the apply command");
        };

        assert_eq!(apply.compute_unit_price, 5);
    }

    #[test]
    fn actions_parses_each_action_without_chain_arguments() {
        let parsed = ["deploy", "verify", "finalize"].map(|name| {
            let arguments = ["actions", "--plan", "plan.json", "--action", name];
            let Command::Actions(actions) = parse(&arguments).unwrap().command else {
                panic!("expected the actions command");
            };

            actions.action
        });

        assert_eq!(
            parsed,
            [ActionKind::Deploy, ActionKind::Verify, ActionKind::Finalize]
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

        let rejected = [
            vec!["actions"],
            vec!["actions", "--plan", "p", "--action", "init"],
            with_chain(&["apply", "--plan", "p"]),
            with_chain(&[
                "plan",
                "--version",
                "1",
                "--cluster",
                "testnet",
                "--program-ids",
                "i",
                "--assets",
                "a",
                "--out",
                "o",
            ]),
        ];

        rejected
            .iter()
            .for_each(|arguments| assert!(parse(arguments).is_err(), "{arguments:?}"));
    }
}
