//! `omnidl-cli serve`: the web interface for servers.

use std::process::ExitCode;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(_args: Args) -> ExitCode {
    eprintln!("omnidl-cli serve: noch nicht fertig");
    ExitCode::FAILURE
}
