//! The terminal program as a user runs it.

#![cfg(feature = "cli")]

use std::path::PathBuf;
use std::process::{Command, Output};

/// "Me at the zoo", 19 seconds, the first YouTube video.
const ZOO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_omnidl-cli")).args(args).output().expect("omnidl-cli startet")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("omnidl-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn text_instead_of_a_link_is_a_usage_error() {
    let out = cli(&["get", ZOO, "Katzenvideo"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("Kein gültiger Link: Katzenvideo"), "{}", text(&out.stderr));
    assert!(out.stdout.is_empty(), "nichts gestartet");
}

#[test]
fn wrong_flags_are_usage_errors() {
    for args in [&["get", ZOO, "--at", "25:00"][..], &["get", ZOO, "-a", "-f", "mp4"], &["get"]] {
        assert_eq!(cli(args).status.code(), Some(2), "{args:?}");
    }
}

#[test]
#[ignore = "braucht Netzwerk"]
fn downloads_me_at_the_zoo_as_mp3() {
    let dir = temp("cli-zoo");
    let out = cli(&["get", ZOO, "-f", "mp3", "--quiet", "-o", dir.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    // --quiet prints just the finished files.
    let path = PathBuf::from(text(&out.stdout).trim());
    assert_eq!(path.extension().unwrap(), "mp3");
    assert!(path.starts_with(&dir), "{path:?}");
    assert!(std::fs::metadata(&path).unwrap().len() > 100_000, "echte Audiodatei");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "braucht Netzwerk"]
fn a_missing_video_fails_with_exit_code_1() {
    let dir = temp("cli-missing");
    let out = cli(&["get", "https://www.youtube.com/watch?v=xxxxxxxxxxx", "--json", "-o", dir.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = text(&out.stdout);
    let last: serde_json::Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(last["event"], "summary");
    assert_eq!((last["done"].as_u64(), last["failed"].as_u64(), last["exit"].as_u64()), (Some(0), Some(1), Some(1)));
    let _ = std::fs::remove_dir_all(&dir);
}
