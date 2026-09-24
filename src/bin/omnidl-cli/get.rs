//! `omnidl-cli get`: download links from the terminal.

use std::process::ExitCode;

#[derive(clap::Args)]
pub struct Args {
    /// Links (YouTube, TikTok, Spotify, …).
    #[arg(required = true)]
    pub urls: Vec<String>,
}

pub fn run(_args: Args) -> ExitCode {
    eprintln!("omnidl-cli get: noch nicht fertig");
    ExitCode::FAILURE
}
