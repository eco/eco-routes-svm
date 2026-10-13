//! Reference relayer for cctp-prover (E2E and manual relays; the production
//! relayer lives in the solver). Keypairs are read from file paths; the RPC
//! URL can come from `SOLANA_RPC_URL` and is never printed.
mod iris;
mod lookup_table;
mod receive;
mod reclaim;
mod rpc;

use clap::{Parser, Subcommand};
use solana_sdk::pubkey::Pubkey;

#[derive(Parser)]
struct Cli {
    #[arg(long, env = "SOLANA_RPC_URL", hide_env_values = true)]
    rpc_url: String,
    /// Path to the signer's keypair file.
    #[arg(long)]
    keypair: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Deliver an attested EVM → Solana message to cctp-prover.
    Receive {
        #[arg(long)]
        source_domain: u32,
        #[arg(long)]
        tx: String,
        #[arg(long)]
        lookup_table: Pubkey,
        #[arg(long, default_value = iris::MAINNET)]
        iris: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Create, fill and freeze the lookup table `receive` uses.
    CreateLookupTable,
    /// Reclaim this signer's `send_message` event accounts older than 5 days.
    ReclaimEventAccounts {
        #[arg(long, default_value = iris::MAINNET)]
        iris: String,
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Receive {
            source_domain,
            tx,
            lookup_table,
            iris,
            dry_run,
        } => receive::run(
            &cli.rpc_url,
            &cli.keypair,
            &iris,
            source_domain,
            &tx,
            lookup_table,
            dry_run,
        ),
        Command::CreateLookupTable => lookup_table::run(&cli.rpc_url, &cli.keypair),
        Command::ReclaimEventAccounts { iris, dry_run } => {
            reclaim::run(&cli.rpc_url, &cli.keypair, &iris, dry_run)
        }
    };
    if let Err(error) = result {
        // The single exit path for failures: redact here too, whatever the module did.
        eprintln!("error: {}", rpc::redact(error, &cli.rpc_url));
        std::process::exit(1);
    }
}
