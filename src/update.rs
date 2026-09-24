//! Keeps omnidl itself current: looks for a newer release on GitHub, loads the
//! desktop app built for this platform, checks it against the published
//! SHA-256 and swaps it in. `omnidl-cli`, when it sits next to the app (MSI,
//! Linux archive), is replaced along with it so both stay on one version.
//!
//! A running program cannot be overwritten on Windows, but it can be renamed;
//! on macOS and Linux renaming over a running binary is fine as well.

use crate::util;
use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const REPO: &str = "Hattinga/omnidl";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The terminal program; it is not updated on its own, only next to the app.
const CLI_NAME: &str = "omnidl-cli";
/// Points the updater at another server, for testing the whole flow.
const URL_OVERRIDE: &str = "OMNIDL_UPDATE_URL";

/// Release assets for one platform: the desktop app and `omnidl-cli`, each
/// published with a `<name>.sha256` next to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssetNames {
    pub app: &'static str,
    pub cli: &'static str,
}

/// What a release has to contain for `os`/`arch` (`std::env::consts` values).
/// Windows keeps the plain `omnidl.exe`: 0.2 clients look for exactly that name.
pub fn asset_names(os: &str, arch: &str) -> Option<AssetNames> {
    let (app, cli) = match (os, arch) {
        // x64 build; Windows on ARM runs it emulated.
        ("windows", _) => ("omnidl.exe", "omnidl-cli.exe"),
        // One universal binary for Apple Silicon and Intel.
        ("macos", _) => ("omnidl-macos-universal", "omnidl-cli-macos-universal"),
        ("linux", "x86_64") => ("omnidl-linux-x86_64", "omnidl-cli-linux-x86_64"),
        ("linux", "aarch64") => ("omnidl-linux-aarch64", "omnidl-cli-linux-aarch64"),
        _ => return None,
    };
    Some(AssetNames { app, cli })
}

fn this_platform() -> Option<AssetNames> {
    asset_names(std::env::consts::OS, std::env::consts::ARCH)
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Asset {
    url: String,
    sha_url: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Release {
    pub version: String,
    pub page: String,
    app: Asset,
    /// Missing in releases that do not ship `omnidl-cli` for this platform.
    cli: Option<Asset>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
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

/// Reads GitHub's release JSON for this platform.
pub fn parse_release(json: &serde_json::Value) -> Option<Release> {
    parse_release_for(json, this_platform()?)
}

/// Drafts, pre-releases and releases without the app for this platform and
/// its checksum are not offered.
pub fn parse_release_for(json: &serde_json::Value, names: AssetNames) -> Option<Release> {
    let flag = |k: &str| json.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    if flag("draft") || flag("prerelease") {
        return None;
    }
    let tag = json.get("tag_name")?.as_str()?;
    parse_version(tag)?;
    let url = |name: &str| {
        json.get("assets")?.as_array()?.iter().find_map(|a| {
            (a.get("name")?.as_str()? == name).then(|| a.get("browser_download_url")?.as_str().map(String::from))?
        })
    };
    let asset = |name: &str| Some(Asset { url: url(name)?, sha_url: url(&format!("{name}.sha256"))? });
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: json.get("html_url").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        app: asset(names.app)?,
        cli: asset(names.cli),
    })
}

/// A newer release, if there is one. When omnidl cannot replace itself (a
/// package manager put it into a system folder), that is an error: the manual
/// check shows it, the automatic one stays silent.
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
    if !is_newer(&rel.version, VERSION) {
        return Ok(None);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Err(e) = can_replace(&exe)
    {
        bail!("Version {} ist verfügbar. {e}", rel.version);
    }
    Ok(Some(rel))
}

/// Whether the updater may swap `exe`: it must be the desktop app, in a
/// folder this user can write to.
pub fn can_replace(exe: &Path) -> Result<()> {
    if exe.file_stem().is_some_and(|s| s == CLI_NAME) {
        bail!("omnidl-cli aktualisiert sich nicht selbst, sondern zusammen mit der App, über den Paketmanager oder Docker.");
    }
    let dir = exe.parent().ok_or_else(|| anyhow!("Programmordner unbekannt"))?;
    if !dir_writable(dir) {
        bail!(
            "omnidl darf {} nicht ändern. Bitte über den Paketmanager aktualisieren oder die neue Version von github.com/{REPO} laden.",
            dir.display()
        );
    }
    Ok(())
}

/// Tries to create (and removes) a file in `dir`; permission bits alone do not
/// tell (ACLs, read-only mounts, macOS App Translocation).
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".omnidl-update-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
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

/// Whether `head` (the first bytes of a file) starts a program that runs on
/// `os`/`arch`: PE on Windows, Mach-O (thin or universal) on macOS, ELF for
/// the right processor on Linux. Catches HTML error pages and wrong downloads
/// that happen to match their checksum file.
pub fn is_native_executable(head: &[u8], os: &str, arch: &str) -> bool {
    match os {
        "windows" => head.starts_with(b"MZ"),
        "macos" => {
            const MAGIC: [[u8; 4]; 6] = [
                [0xcf, 0xfa, 0xed, 0xfe], // 64-bit, little-endian (arm64, x86_64)
                [0xce, 0xfa, 0xed, 0xfe], // 32-bit, little-endian
                [0xfe, 0xed, 0xfa, 0xcf], // 64-bit, big-endian
                [0xfe, 0xed, 0xfa, 0xce], // 32-bit, big-endian
                [0xca, 0xfe, 0xba, 0xbe], // universal
                [0xca, 0xfe, 0xba, 0xbf], // universal, 64-bit offsets
            ];
            head.len() >= 4 && MAGIC.iter().any(|m| head[..4] == *m)
        }
        _ => {
            if !head.starts_with(b"\x7fELF") || head.len() < 20 {
                return false;
            }
            // e_machine, in the file's byte order.
            let machine = match head[5] {
                1 => u16::from_le_bytes([head[18], head[19]]),
                2 => u16::from_be_bytes([head[18], head[19]]),
                _ => return false,
            };
            match arch {
                "x86_64" => machine == 62,
                "aarch64" => machine == 183,
                _ => true,
            }
        }
    }
}

fn with_suffix(exe: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(exe.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

/// `omnidl.exe` → `omnidl.exe.new`, `omnidl` → `omnidl.new`.
pub fn new_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".new")
}

pub fn old_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".old")
}

/// `omnidl-cli` next to the app, if it is there.
fn cli_next_to(exe: &Path) -> Option<PathBuf> {
    let cli = exe.with_file_name(format!("{CLI_NAME}{}", std::env::consts::EXE_SUFFIX));
    cli.is_file().then_some(cli)
}

/// Loads the release next to `exe`, checks it and swaps it in (and
/// `omnidl-cli`, if it lives in the same folder).
pub async fn install(rel: &Release, exe: &Path, mut progress: impl FnMut(Option<f32>)) -> Result<()> {
    can_replace(exe)?;
    let cli = cli_next_to(exe).zip(rel.cli.as_ref());
    // The app is the bigger file; its download fills most of the bar.
    let app_share = if cli.is_some() { 0.7 } else { 1.0 };

    let app_new = new_path(exe);
    download(&rel.app, &app_new, |f| progress(f.map(|f| f * app_share))).await?;
    if let Some((cli_exe, asset)) = &cli {
        if let Err(e) = download(asset, &new_path(cli_exe), |f| progress(f.map(|f| app_share + f * (1.0 - app_share)))).await {
            let _ = std::fs::remove_file(&app_new);
            return Err(e.context("omnidl-cli"));
        }
    }
    progress(Some(1.0));

    swap(exe, &app_new).context("omnidl konnte nicht ersetzt werden")?;
    if let Some((cli_exe, _)) = &cli {
        swap(cli_exe, &new_path(cli_exe)).context("omnidl-cli konnte nicht ersetzt werden")?;
    }
    Ok(())
}

/// Downloads `asset` to `dest` and checks checksum and file type; a bad file
/// is deleted again.
async fn download(asset: &Asset, dest: &Path, mut progress: impl FnMut(Option<f32>)) -> Result<()> {
    let http = util::http();
    let sums = http.get(&asset.sha_url).send().await?.error_for_status()?.text().await?;
    let expected = parse_checksum(&sums).ok_or_else(|| anyhow!("Prüfsumme unlesbar"))?;

    let resp = http.get(&asset.url).send().await?.error_for_status().context("Download fehlgeschlagen")?;
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest).await.context("Datei im Programmordner nicht anlegbar")?;
    let mut hasher = Sha256::new();
    let mut head = Vec::new();
    let mut got = 0u64;
    let mut last = std::time::Instant::now();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if head.len() < 64 {
            head.extend_from_slice(&chunk[..chunk.len().min(64 - head.len())]);
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
    if actual != expected {
        let _ = tokio::fs::remove_file(dest).await;
        bail!("Die geladene Datei ist beschädigt (Prüfsumme stimmt nicht)");
    }
    if !is_native_executable(&head, std::env::consts::OS, std::env::consts::ARCH) {
        let _ = tokio::fs::remove_file(dest).await;
        bail!("Die geladene Datei ist kein Programm für dieses System");
    }
    make_executable(dest)?;
    progress(Some(1.0));
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
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
    if let Some(cli) = cli_next_to(exe) {
        let _ = std::fs::remove_file(old_path(&cli));
        let _ = std::fs::remove_file(new_path(&cli));
    }
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

/// Linux reports the running binary under its new name after the swap
/// (`omnidl.old`, or `omnidl.old (deleted)` once the new instance cleaned
/// up); the program to start is the one under the original name.
pub fn original_exe(exe: &Path) -> PathBuf {
    let s = exe.to_string_lossy();
    let s = s.strip_suffix(" (deleted)").unwrap_or(&s);
    PathBuf::from(s.strip_suffix(".old").unwrap_or(s))
}

/// Starts the (new) exe; it waits until this instance has let go of the port.
pub fn restart(exe: &Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(original_exe(exe));
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

    const WINDOWS: AssetNames = AssetNames { app: "omnidl.exe", cli: "omnidl-cli.exe" };

    #[test]
    fn release_needs_exe_and_checksum() {
        let rel = parse_release_for(
            &release_json("v0.3.0", &["omnidl-0.3.0-x64.msi", "omnidl.exe", "omnidl.exe.sha256"]),
            WINDOWS,
        )
        .unwrap();
        assert_eq!(rel.version, "0.3.0");
        assert_eq!(rel.app.url, "https://example.com/omnidl.exe");
        assert_eq!(rel.app.sha_url, "https://example.com/omnidl.exe.sha256");
        assert_eq!(rel.cli, None, "ältere Releases ohne omnidl-cli bleiben gültig");
        assert!(rel.page.ends_with("/v0.3.0"));

        assert!(parse_release_for(&release_json("v0.3.0", &["omnidl.exe"]), WINDOWS).is_none(), "ohne Prüfsumme kein Update");
        let mut pre = release_json("v0.4.0", &["omnidl.exe", "omnidl.exe.sha256"]);
        pre["prerelease"] = true.into();
        assert!(parse_release_for(&pre, WINDOWS).is_none());
    }

    #[test]
    fn cli_comes_along_only_with_its_checksum() {
        let all = ["omnidl.exe", "omnidl.exe.sha256", "omnidl-cli.exe", "omnidl-cli.exe.sha256"];
        let rel = parse_release_for(&release_json("v0.3.0", &all), WINDOWS).unwrap();
        let cli = rel.cli.expect("omnidl-cli");
        assert_eq!(cli.url, "https://example.com/omnidl-cli.exe");
        assert_eq!(cli.sha_url, "https://example.com/omnidl-cli.exe.sha256");

        let rel = parse_release_for(&release_json("v0.3.0", &all[..3]), WINDOWS).unwrap();
        assert_eq!(rel.cli, None);
    }

    #[test]
    fn every_platform_gets_its_own_asset() {
        let pick = |os, arch| asset_names(os, arch).map(|n| (n.app, n.cli));
        assert_eq!(pick("windows", "x86_64"), Some(("omnidl.exe", "omnidl-cli.exe")), "Namen von 0.2 bleiben");
        assert_eq!(pick("windows", "aarch64"), Some(("omnidl.exe", "omnidl-cli.exe")));
        assert_eq!(pick("macos", "aarch64"), Some(("omnidl-macos-universal", "omnidl-cli-macos-universal")));
        assert_eq!(pick("macos", "x86_64"), pick("macos", "aarch64"));
        assert_eq!(pick("linux", "x86_64"), Some(("omnidl-linux-x86_64", "omnidl-cli-linux-x86_64")));
        assert_eq!(pick("linux", "aarch64"), Some(("omnidl-linux-aarch64", "omnidl-cli-linux-aarch64")));
        assert_eq!(pick("linux", "riscv64"), None);
        assert_eq!(pick("freebsd", "x86_64"), None);
        assert!(this_platform().is_some(), "gebaute Plattformen haben ein Asset");

        // A full release offers each system its own file.
        let mut names = vec![];
        for (os, arch) in [("windows", "x86_64"), ("macos", "aarch64"), ("linux", "x86_64"), ("linux", "aarch64")] {
            let n = asset_names(os, arch).unwrap();
            for a in [n.app, n.cli] {
                names.push(a.to_string());
                names.push(format!("{a}.sha256"));
            }
        }
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let json = release_json("v0.3.0", &names);
        let linux_arm = parse_release_for(&json, asset_names("linux", "aarch64").unwrap()).unwrap();
        assert_eq!(linux_arm.app.url, "https://example.com/omnidl-linux-aarch64");
        assert_eq!(linux_arm.cli.unwrap().sha_url, "https://example.com/omnidl-cli-linux-aarch64.sha256");
        let mac = parse_release_for(&json, asset_names("macos", "x86_64").unwrap()).unwrap();
        assert_eq!(mac.app.url, "https://example.com/omnidl-macos-universal");

        // A Windows-only release (0.2) offers nothing to Linux.
        let win_only = release_json("v0.3.0", &["omnidl.exe", "omnidl.exe.sha256"]);
        assert!(parse_release_for(&win_only, asset_names("linux", "x86_64").unwrap()).is_none());
    }

    fn elf(data: u8, machine: u16) -> Vec<u8> {
        let mut h = vec![0x7f, b'E', b'L', b'F', 2, data, 1, 0];
        h.resize(18, 0);
        h.extend(if data == 2 { machine.to_be_bytes() } else { machine.to_le_bytes() });
        h.resize(64, 0);
        h
    }

    #[test]
    fn executables_are_recognised_per_platform() {
        let pe = b"MZ\x90\x00\x03\x00\x00\x00".to_vec();
        let macho_arm = [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01].to_vec();
        let fat = [0xca, 0xfe, 0xba, 0xbe, 0x00, 0x00, 0x00, 0x02].to_vec();
        let elf_x64 = elf(1, 62);
        let elf_arm = elf(1, 183);
        let html = b"<!DOCTYPE html><html>".to_vec();

        assert!(is_native_executable(&pe, "windows", "x86_64"));
        assert!(!is_native_executable(&elf_x64, "windows", "x86_64"));
        assert!(!is_native_executable(&html, "windows", "x86_64"));

        assert!(is_native_executable(&macho_arm, "macos", "aarch64"));
        assert!(is_native_executable(&fat, "macos", "x86_64"), "Universal-Binary");
        assert!(!is_native_executable(&pe, "macos", "aarch64"));
        assert!(!is_native_executable(&elf_arm, "macos", "aarch64"));
        assert!(!is_native_executable(&fat[..3], "macos", "aarch64"), "zu kurz");

        assert!(is_native_executable(&elf_x64, "linux", "x86_64"));
        assert!(is_native_executable(&elf_arm, "linux", "aarch64"));
        assert!(!is_native_executable(&elf_arm, "linux", "x86_64"), "falscher Prozessor");
        assert!(!is_native_executable(&elf_x64, "linux", "aarch64"));
        assert!(is_native_executable(&elf(2, 62), "linux", "x86_64"), "Big-Endian-Header");
        assert!(!is_native_executable(&elf_x64[..10], "linux", "x86_64"));
        assert!(!is_native_executable(&macho_arm, "linux", "aarch64"));
        assert!(!is_native_executable(&html, "linux", "x86_64"));
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
    fn side_files_keep_the_full_name() {
        assert_eq!(new_path(Path::new("dir/omnidl.exe")), Path::new("dir/omnidl.exe.new"), "wie in 0.2");
        assert_eq!(old_path(Path::new("dir/omnidl.exe")), Path::new("dir/omnidl.exe.old"));
        assert_eq!(new_path(Path::new("dir/omnidl")), Path::new("dir/omnidl.new"));
        assert_eq!(old_path(Path::new("dir/omnidl-cli")), Path::new("dir/omnidl-cli.old"));
    }

    #[test]
    fn restart_uses_the_original_name() {
        assert_eq!(original_exe(Path::new("/opt/omnidl/omnidl")), Path::new("/opt/omnidl/omnidl"));
        assert_eq!(original_exe(Path::new("/opt/omnidl/omnidl.old")), Path::new("/opt/omnidl/omnidl"));
        assert_eq!(original_exe(Path::new("/opt/omnidl/omnidl.old (deleted)")), Path::new("/opt/omnidl/omnidl"));
        assert_eq!(original_exe(Path::new(r"C:\x\omnidl.exe")), Path::new(r"C:\x\omnidl.exe"));
    }

    #[test]
    fn only_the_app_in_a_writable_folder_replaces_itself() {
        let dir = std::env::temp_dir().join("omnidl-tests").join("replace");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(can_replace(&dir.join("omnidl.exe")).is_ok());
        assert!(can_replace(&dir.join("omnidl")).is_ok());
        let err = can_replace(&dir.join(format!("omnidl-cli{}", std::env::consts::EXE_SUFFIX))).unwrap_err();
        assert!(err.to_string().contains("omnidl-cli"));
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "Probedatei wieder entfernt");

        let missing = dir.join("gibt-es-nicht").join("omnidl");
        let err = can_replace(&missing).unwrap_err().to_string();
        assert!(err.contains("Paketmanager"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn read_only_folder_asks_for_the_package_manager() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join("omnidl-tests").join("read-only");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable = dir_writable(&dir); // root may write anyway
        let result = can_replace(&dir.join("omnidl"));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(result.is_ok(), writable);
        if !writable {
            assert!(result.unwrap_err().to_string().contains("Paketmanager"));
        }
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
    fn cleanup_includes_the_cli_next_to_the_app() {
        let dir = std::env::temp_dir().join("omnidl-tests").join("cleanup-cli");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("omnidl.exe");
        let cli = dir.join(format!("omnidl-cli{}", std::env::consts::EXE_SUFFIX));
        for f in [&exe, &cli, &old_path(&cli), &new_path(&cli)] {
            std::fs::write(f, "x").unwrap();
        }
        assert_eq!(cli_next_to(&exe).as_deref(), Some(cli.as_path()));
        cleanup(&exe);
        assert!(!old_path(&cli).exists() && !new_path(&cli).exists());
        assert!(cli.exists());
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
