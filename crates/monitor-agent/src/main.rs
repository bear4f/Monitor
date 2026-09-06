use std::process::ExitCode;

use monitor_agent::{cli, runtime};

fn main() -> ExitCode {
    match cli::parse(std::env::args()) {
        Ok(cli::Action::Help) => {
            println!("{}", cli::usage());
            ExitCode::SUCCESS
        }
        Ok(cli::Action::Version) => {
            println!("monitor-agent {}", monitor_agent::VERSION);
            ExitCode::SUCCESS
        }
        Ok(cli::Action::Run(config)) => match runtime::run(config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("monitor-agent: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("monitor-agent: {error}\n\n{}", cli::usage());
            ExitCode::FAILURE
        }
    }
}
