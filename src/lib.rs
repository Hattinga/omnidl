//! omnidl's core: everything except the window. The desktop app (`omnidl`)
//! and the terminal/server program (`omnidl-cli`) are both built on it.

pub mod bridge;
pub mod config;
pub mod deps;
pub mod detect;
pub mod engine;
pub mod extension;
pub mod gallery;
pub mod job;
pub mod jobs;
pub mod journal;
pub mod launch;
pub mod matcher;
pub mod peek;
pub mod schedule;
pub mod spotify;
pub mod sys;
pub mod tagger;
pub mod update;
pub mod util;
pub mod ytdlp;
