//! Keeps omnidl itself current: looks for a newer release on GitHub, loads
//! `omnidl.exe`, checks it against the published SHA-256 and swaps it in.
//! A running exe cannot be overwritten on Windows, but it can be renamed.

use crate::util;
use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const REPO: &str = "Hattinga/omnidl";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const EXE_ASSET: &str = "omnidl.exe";
const SHA_ASSET: &str = "omnidl.exe.sha256";
/// Points the updater at another server, for testing the whole flow.
const URL_OVERRIDE: &str = "OMNIDL_UPDATE_URL";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    exe_url: String,
    sha_url: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading { version: String, frac: Option<f32> },
    /// Swapped in; takes effect after a restart.
    Ready(String),
    Failed(String),
}

/// Automatic checks only run in release builds, or when pointed at a test server.
pub fn auto_enabled() -> bool {
    !cfg!(debug_assertions) || std::env::var_os(URL_OVERRIDE).is_some()
}

fn latest_url() -> String {
    std::env::var(URL_OVERRIDE).unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"))
}

/// `v0.3.1` → (0, 3, 1). Pre-release suffixes are ignored.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Reads GitHub's release JSON. Drafts, pre-releases and releases without the
/// exe and its checksum are not offered.
pub fn parse_release(json: &serde_json::Value) -> Option<Release> {
    let flag = |k: &str| json.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    if flag("draft") || flag("prerelease") {
        return None;
    }
    let tag = json.get("tag_name")?.as_str()?;
    parse_version(tag)?;
    let asset = |name: &str| {
        json.get("assets")?.as_array()?.iter().find_map(|a| {
            (a.get("name")?.as_str()? == name).then(|| a.get("browser_download_url")?.as_str().map(String::from))?
        })
    };
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: json.get("html_url").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        exe_url: asset(EXE_ASSET)?,
        sha_url: asset(SHA_ASSET)?,
    })
}

/// A newer release, if there is one.
pub async fn check() -> Result<Option<Release>> {
    let resp = util::http()
        .get(latest_url())
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("GitHub nicht erreichbar")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        // No release published yet.
        return Ok(None);
    }
    let json: serde_json::Value = resp.error_for_status().context("GitHub antwortet mit Fehler")?.json().await?;
    let Some(rel) = parse_release(&json) else { return Ok(None) };
    Ok(is_newer(&rel.version, VERSION).then_some(rel))
}

/// `sha256sum` style: the first 64-digit hex word.
pub fn parse_checksum(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_start_matches('*'))
        .find(|w| w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|w| w.to_ascii_lowercase())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn new_path(exe: &Path) -> PathBuf {
    exe.with_extension("exe.new")
}

pub fn old_path(exe: &Path) -> PathBuf {
    exe.with_extension("exe.old")
}

/// Loads the release next to `exe`, checks it and swaps it in.
pub async fn install(rel: &Release, exe: &Path, mut progress: impl FnMut(Option<f32>)) -> Result<()> {
    let http = util::http();
    let sums = http.get(&rel.sha_url).send().await?.error_for_status()?.text().await?;
    let expected = parse_checksum(&sums).ok_or_else(|| anyhow!("Prüfsumme unlesbar"))?;

    let resp = http.get(&rel.exe_url).send().await?.error_for_status().context("Download fehlgeschlagen")?;
    let total = resp.content_length();
    let new = new_path(exe);
    let mut file = tokio::fs::File::create(&new).await.context("Datei im Programmordner nicht anlegbar")?;
    let mut hasher = Sha256::new();
    let mut head = Vec::new();
    let mut got = 0u64;
    let mut last = std::time::Instant::now();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if head.len() < 2 {
            head.extend_from_slice(&chunk[..chunk.len().min(2)]);
        }
        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        got += chunk.len() as u64;
        if last.elapsed().as_millis() > 150 {
            last = std::time::Instant::now();
            progress(total.map(|t| got as f32 / t.max(1) as f32));
        }
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);

    let actual = hex(&hasher.finalize());
    if actual != expected || !head.starts_with(b"MZ") {
        let _ = tokio::fs::remove_file(&new).await;
        bail!("Die geladene Datei ist beschädigt (Prüfsumme stimmt nicht)");
    }
    progress(Some(1.0));
    swap(exe, &new).context("omnidl.exe konnte nicht ersetzt werden")
}

/// Puts `new` in place of the running `exe`; the old one waits as `.old`
/// until the next start deletes it.
pub fn swap(exe: &Path, new: &Path) -> std::io::Result<()> {
    let old = old_path(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

/// Removes what an earlier update left behind.
pub fn cleanup(exe: &Path) {
    let _ = std::fs::remove_file(old_path(exe));
    let _ = std::fs::remove_file(new_path(exe));
}

/// After an update the old instance may still be exiting and hold its exe
/// for a moment; keep trying briefly.
pub fn cleanup_when_free(exe: &Path) {
    for _ in 0..40 {
        cleanup(exe);
        if !old_path(exe).exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Starts the (new) exe; it waits until this instance has let go of the port.
pub fn restart(exe: &Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(crate::launch::RESTARTED);
    crate::sys::spawn_detached(&mut cmd).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("latest"), None);
        assert!(is_newer("v0.10.0", "0.9.9"), "nicht als Text vergleichen");
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("kaputt", "0.2.0"));
    }

    fn release_json(tag: &str, assets: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/{REPO}/releases/tag/{tag}"),
            "draft": false,
            "prerelease": false,
            "assets": assets.iter().map(|n| serde_json::json!({
                "name": n,
                "browser_download_url": format!("https://example.com/{n}"),
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn release_needs_exe_and_checksum() {
        let rel = parse_release(&release_json("v0.3.0", &["omnidl-0.3.0-x64.msi", "omnidl.exe", "omnidl.exe.sha256"])).unwrap();
        assert_eq!(rel.version, "0.3.0");
        assert_eq!(rel.exe_url, "https://example.com/omnidl.exe");
        assert_eq!(rel.sha_url, "https://example.com/omnidl.exe.sha256");
        assert!(rel.page.ends_with("/v0.3.0"));

        assert!(parse_release(&release_json("v0.3.0", &["omnidl.exe"])).is_none(), "ohne Prüfsumme kein Update");
        let mut pre = release_json("v0.4.0", &["omnidl.exe", "omnidl.exe.sha256"]);
        pre["prerelease"] = true.into();
        assert!(parse_release(&pre).is_none());
    }

    #[test]
    fn checksum_files_in_common_shapes() {
        let h = "A".repeat(64);
        assert_eq!(parse_checksum(&format!("{h}  omnidl.exe\n")), Some("a".repeat(64)));
        assert_eq!(parse_checksum(&format!("{h} *omnidl.exe")), Some("a".repeat(64)));
        assert_eq!(parse_checksum(&format!("SHA256 (omnidl.exe) = {}", "0f".repeat(32))), Some("0f".repeat(32)));
        assert_eq!(parse_checksum("nichts"), None);
    }

    #[test]
    fn swap_replaces_and_keeps_the_old_one_aside() {
        let dir = std::env::temp_dir().join("omnidl-tests").join("swap");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("omnidl.exe");
        std::fs::write(&exe, "alt").unwrap();
        std::fs::write(new_path(&exe), "neu").unwrap();
        std::fs::write(old_path(&exe), "vorletzte").unwrap();

        swap(&exe, &new_path(&exe)).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "neu");
        assert_eq!(std::fs::read_to_string(old_path(&exe)).unwrap(), "alt");
        assert!(!new_path(&exe).exists());

        cleanup(&exe);
        assert!(!old_path(&exe).exists());
        assert!(exe.exists());
    }

    #[test]
    fn failed_swap_restores_the_running_exe() {
        let dir = std::env::temp_dir().join("omnidl-tests").join("swap-fail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("omnidl.exe");
        std::fs::write(&exe, "alt").unwrap();
        assert!(swap(&exe, &dir.join("fehlt.exe")).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "alt", "nichts kaputt gemacht");
    }
}
