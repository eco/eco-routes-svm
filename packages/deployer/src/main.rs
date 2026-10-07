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
            eprintln!("{error:?}");

            ExitCode::FAILURE
        }
    }
}
