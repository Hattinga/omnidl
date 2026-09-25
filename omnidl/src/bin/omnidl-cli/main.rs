//! omnidl without a window: downloads in the terminal (`get`) or a web
//! interface for servers and NAS boxes (`serve`). Needs no graphics libraries.

mod get;
mod term;
mod tools;
mod web;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "omnidl-cli",
    version,
    about = "omnidl ohne Fenster: Downloads im Terminal oder als Web-Interface für Server."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lädt Links herunter und beendet sich, wenn alles fertig ist.
    Get(get::Args),
    /// Startet das Web-Interface (für Server, NAS, Heimnetz).
    Serve(web::Args),
    /// Zeigt oder aktualisiert die Werkzeuge (yt-dlp, ffmpeg, Deno, gallery-dl).
    Tools(tools::Args),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Get(args) => get::run(args),
        Command::Serve(args) => web::run(args),
        Command::Tools(args) => tools::run(args),
    }
}
