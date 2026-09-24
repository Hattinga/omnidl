//! `omnidl-cli tools`: show or update yt-dlp, ffmpeg, Deno and gallery-dl.

use std::process::ExitCode;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(_args: Args) -> ExitCode {
    eprintln!("omnidl-cli tools: noch nicht fertig");
    ExitCode::FAILURE
}
