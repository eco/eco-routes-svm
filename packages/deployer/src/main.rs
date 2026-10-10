use std::io::stdout;
use std::process::ExitCode;

use clap::Parser;
use deployer::cli::Cli;
use deployer::commands;

fn main() -> ExitCode {
    let Cli { command } = Cli::parse();

    match commands::run(&command, &mut stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The message says what to do next; the debug form adds the variant and its sources.
            eprintln!("error: {error}\n{error:?}");

            ExitCode::FAILURE
        }
    }
}
