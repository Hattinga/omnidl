use crate::util;
use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};

/// External binaries the app drives. Downloaded ones live in `<base>/bin`. On
/// macOS and Linux a program already installed (Homebrew, apt, …) stands in
/// for a missing download, so `ensure` does not fetch what is already there.
pub struct Tools {
    pub bin: PathBuf,
}

impl Tools {
    pub fn new(base: &Path) -> Self {
        Self { bin: base.join("bin") }
    }

    pub fn ytdlp(&self) -> PathBuf {
        self.pick(self.bin.join("yt-dlp").join(exe("yt-dlp")), "yt-dlp")
    }

    /// Folder holding ffmpeg and ffprobe, for yt-dlp's `--ffmpeg-location`.
    pub fn ffmpeg_dir(&self) -> PathBuf {
        let both = |dir: &Path| dir.join(exe("ffmpeg")).is_file() && dir.join(exe("ffprobe")).is_file();
        if cfg!(windows) || both(&self.bin) {
            return self.bin.clone();
        }
        search_path().into_iter().find(|d| both(d)).unwrap_or_else(|| self.bin.clone())
    }
    pub fn ffmpeg(&self) -> PathBuf {
        self.ffmpeg_dir().join(exe("ffmpeg"))
    }
    pub fn ffprobe(&self) -> PathBuf {
        self.ffmpeg_dir().join(exe("ffprobe"))
    }
    pub fn deno(&self) -> PathBuf {
        self.pick(self.bin.join(exe("deno")), "deno")
    }
    pub fn gallery_dl(&self) -> PathBuf {
        self.pick(self.bin.join(exe("gallery-dl")), "gallery-dl")
    }

    /// Value for yt-dlp `--js-runtimes`. YouTube needs one since late 2025.
    pub fn js_runtime(&self) -> Option<String> {
        let deno = self.deno();
        if deno.is_file() {
            return Some(format!("deno:{}", deno.display()));
        }
        find_program("node").map(|node| format!("node:{}", node.display()))
    }

    /// The downloaded copy, else (not on Windows) an installed one. Without
    /// either it is the download location, which `ensure` fills.
    fn pick(&self, local: PathBuf, name: &str) -> PathBuf {
        if local.is_file() || cfg!(windows) {
            return local;
        }
        find_program(name).unwrap_or(local)
    }
}

/// `name` with the platform's executable suffix (`.exe` on Windows).
fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Folders searched for installed programs: PATH, plus the Homebrew and
/// MacPorts folders that apps started from the Finder do not get, and
/// `~/.local/bin` (pipx).
fn search_path() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(target_os = "macos") {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].map(PathBuf::from));
    }
    if cfg!(unix) {
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local").join("bin"));
        }
    }
    dirs
}

fn find_program(name: &str) -> Option<PathBuf> {
    find_in(&search_path(), name)
}

fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let file = exe(name);
    dirs.iter().map(|d| d.join(&file)).find(|p| is_program(p))
}

fn is_program(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct DepsStatus {
    pub ytdlp: Option<String>,
    /// Days since yt-dlp was last refreshed, at most the age of its release.
    pub ytdlp_age_days: Option<i64>,
    pub ffmpeg: bool,
    pub js: Option<String>,
    pub gallery: bool,
    pub busy: Option<String>,
    pub error: Option<String>,
}

impl DepsStatus {
    pub fn ready(&self) -> bool {
        self.ytdlp.is_some() && self.ffmpeg && self.busy.is_none()
    }
}

/// Past this many days an installed yt-dlp is replaced by a download: sites
/// change often, and distribution packages lag far behind.
const STALE_DAYS: i64 = 14;

/// Where the tools come from on one platform. `None` where no ready-made
/// build exists; the program then has to be installed by hand.
#[derive(Debug, Default, PartialEq)]
struct Sources {
    /// yt-dlp as a folder zip: starts faster than the single-file builds and
    /// needs no executable temp folder.
    ytdlp: Option<String>,
    ffmpeg: Option<Ffmpeg>,
    /// Zip with the `deno` executable.
    deno: Option<String>,
    /// The executable itself.
    gallery: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Ffmpeg {
    /// One archive (.zip or .tar.xz) with `bin/ffmpeg` and `bin/ffprobe`.
    Bundle(String),
    /// One zip per program.
    Pair { ffmpeg: String, ffprobe: String },
}

impl Sources {
    fn current() -> Self {
        Self::of(std::env::consts::OS, std::env::consts::ARCH, cfg!(target_env = "musl"))
    }

    fn of(os: &str, arch: &str, musl: bool) -> Self {
        let ytdlp = |file: &str| Some(format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{file}"));
        let ffmpeg = |build: &str, ext: &str| {
            Some(Ffmpeg::Bundle(format!(
                "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-{build}-gpl.{ext}"
            )))
        };
        // yt-dlp's FFmpeg builds have no macOS version; Martin Riedl's are signed and static.
        let riedl = |arch: &str| {
            let url = |tool: &str| format!("https://ffmpeg.martin-riedl.de/redirect/latest/macos/{arch}/release/{tool}.zip");
            Some(Ffmpeg::Pair { ffmpeg: url("ffmpeg"), ffprobe: url("ffprobe") })
        };
        let deno = |target: &str| Some(format!("https://github.com/denoland/deno/releases/latest/download/deno-{target}.zip"));
        let gallery = |file: &str| Some(format!("https://codeberg.org/mikf/gallery-dl/releases/download/latest/{file}"));

        match (os, arch, musl) {
            ("windows", "x86_64", _) => Sources {
                ytdlp: ytdlp("yt-dlp_win.zip"),
                ffmpeg: ffmpeg("win64", "zip"),
                deno: deno("x86_64-pc-windows-msvc"),
                gallery: gallery("gallery-dl.exe"),
            },
            ("windows", "aarch64", _) => Sources {
                ytdlp: ytdlp("yt-dlp_win_arm64.zip"),
                ffmpeg: ffmpeg("winarm64", "zip"),
                deno: deno("aarch64-pc-windows-msvc"),
                gallery: gallery("gallery-dl.exe"),
            },
            // yt-dlp_macos is universal; gallery-dl has no macOS build (brew install gallery-dl).
            ("macos", "x86_64", _) => Sources {
                ytdlp: ytdlp("yt-dlp_macos.zip"),
                ffmpeg: riedl("amd64"),
                deno: deno("x86_64-apple-darwin"),
                gallery: None,
            },
            ("macos", "aarch64", _) => Sources {
                ytdlp: ytdlp("yt-dlp_macos.zip"),
                ffmpeg: riedl("arm64"),
                deno: deno("aarch64-apple-darwin"),
                gallery: None,
            },
            ("linux", "x86_64", false) => Sources {
                ytdlp: ytdlp("yt-dlp_linux.zip"),
                ffmpeg: ffmpeg("linux64", "tar.xz"),
                deno: deno("x86_64-unknown-linux-gnu"),
                gallery: gallery("gallery-dl.bin"),
            },
            ("linux", "aarch64", false) => Sources {
                ytdlp: ytdlp("yt-dlp_linux_aarch64.zip"),
                ffmpeg: ffmpeg("linuxarm64", "tar.xz"),
                deno: deno("aarch64-unknown-linux-gnu"),
                gallery: None,
            },
            // Alpine and co.: Deno and gallery-dl are built for glibc only.
            ("linux", "x86_64", true) => Sources {
                ytdlp: ytdlp("yt-dlp_musllinux.zip"),
                ffmpeg: ffmpeg("linux64", "tar.xz"),
                ..Default::default()
            },
            ("linux", "aarch64", true) => Sources {
                ytdlp: ytdlp("yt-dlp_musllinux_aarch64.zip"),
                ffmpeg: ffmpeg("linuxarm64", "tar.xz"),
                ..Default::default()
            },
            _ => Sources::default(),
        }
    }
}

/// Reads versions / presence of every tool.
pub async fn status(tools: &Tools) -> DepsStatus {
    let mut s = DepsStatus {
        ffmpeg: tools.ffmpeg().is_file() && tools.ffprobe().is_file(),
        gallery: tools.gallery_dl().is_file(),
        js: tools.js_runtime().map(|r| {
            if r.starts_with("deno") { "deno".to_string() } else { "node".to_string() }
        }),
        ..Default::default()
    };
    if let Some((version, age)) = ytdlp_info(&tools.ytdlp()).await {
        s.ytdlp = Some(version);
        s.ytdlp_age_days = age;
    }
    s
}

/// Days since the file was written, i.e. since the last install or update.
fn days_since_modified(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(modified).ok()?;
    Some((age.as_secs() / 86_400) as i64)
}

/// Version and age in days. `None` if yt-dlp is missing or does not start
/// (e.g. a damaged install).
async fn ytdlp_info(path: &Path) -> Option<(String, Option<i64>)> {
    if !path.is_file() {
        return None;
    }
    let out = util::command(path).arg("--version").output().await.ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || v.is_empty() {
        return None;
    }
    // A fresh download of the newest release is not stale, however old the release is.
    let refreshed = days_since_modified(path);
    let age = version_age_days(&v).map(|age| refreshed.map_or(age, |d| age.min(d)));
    Some((v, age))
}

/// yt-dlp versions are dates: `2026.08.19`.
fn version_age_days(v: &str) -> Option<i64> {
    let mut it = v.split('.');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    Some(util::today_days() - util::days_from_civil(y, m, d))
}

/// yt-dlp is downloaded when none works, or when the only one is an installed
/// copy that has fallen behind.
async fn needs_ytdlp(tools: &Tools) -> bool {
    let path = tools.ytdlp();
    match ytdlp_info(&path).await {
        None => true,
        Some((_, age)) => !path.starts_with(&tools.bin) && age.is_some_and(|a| a > STALE_DAYS),
    }
}

fn unavailable(name: &str) -> anyhow::Error {
    anyhow!("{name} gibt es für dieses System nicht zum Herunterladen – bitte über die Paketverwaltung installieren")
}

/// Downloads whatever is missing. `progress` reports a human readable status line.
pub async fn ensure(tools: &Tools, force_ytdlp: bool, mut progress: impl FnMut(String)) -> Result<()> {
    tokio::fs::create_dir_all(&tools.bin).await.ok();
    let sources = Sources::current();

    if force_ytdlp || needs_ytdlp(tools).await {
        let url = sources.ytdlp.as_deref().ok_or_else(|| unavailable("yt-dlp"))?;
        let zip = tools.bin.join("yt-dlp.zip");
        fetch(url, &zip, "yt-dlp", &mut progress).await?;
        let dest = tools.bin.join("yt-dlp");
        let tmp = tools.bin.join("yt-dlp.new");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        unzip(&zip, &tmp, None).await?;
        name_ytdlp(&tmp)?;
        let _ = tokio::fs::remove_dir_all(&dest).await;
        tokio::fs::rename(&tmp, &dest)
            .await
            .context("yt-dlp konnte nicht ersetzt werden (läuft noch ein Download?)")?;
        let _ = tokio::fs::remove_file(&zip).await;
    }

    if !tools.ffmpeg().is_file() || !tools.ffprobe().is_file() {
        match sources.ffmpeg.as_ref().ok_or_else(|| unavailable("ffmpeg"))? {
            Ffmpeg::Bundle(url) => {
                let archive = tools.bin.join(if url.ends_with(".tar.xz") { "ffmpeg.tar.xz" } else { "ffmpeg.zip" });
                fetch(url, &archive, "ffmpeg", &mut progress).await?;
                let wanted = vec![format!("bin/{}", exe("ffmpeg")), format!("bin/{}", exe("ffprobe"))];
                if url.ends_with(".tar.xz") {
                    progress("Entpacke ffmpeg …".into());
                    untar_xz(&archive, &tools.bin, wanted).await?;
                } else {
                    unzip(&archive, &tools.bin, Some(wanted)).await?;
                }
                let _ = tokio::fs::remove_file(&archive).await;
            }
            Ffmpeg::Pair { ffmpeg, ffprobe } => {
                install_from_zip(tools, ffmpeg, "ffmpeg", &mut progress).await?;
                install_from_zip(tools, ffprobe, "ffprobe", &mut progress).await?;
            }
        }
    }

    // Without a JavaScript runtime only YouTube suffers; the app warns about it.
    if let Some(url) = &sources.deno {
        if !tools.deno().is_file() && find_program("node").is_none() {
            install_from_zip(tools, url, "deno", &mut progress).await?;
        }
    }

    // gallery-dl is optional; where no build exists it may be installed by hand.
    if let Some(url) = &sources.gallery {
        if !tools.gallery_dl().is_file() {
            let dest = tools.bin.join(exe("gallery-dl"));
            fetch(url, &dest, "gallery-dl", &mut progress).await?;
            make_executable(&dest)?;
        }
    }
    Ok(())
}

/// Downloads a zip and takes the program `name` out of it into `bin/`.
async fn install_from_zip(tools: &Tools, url: &str, name: &str, progress: &mut impl FnMut(String)) -> Result<()> {
    let zip = tools.bin.join(format!("{name}.zip"));
    fetch(url, &zip, name, progress).await?;
    unzip(&zip, &tools.bin, Some(vec![exe(name)])).await?;
    let _ = tokio::fs::remove_file(&zip).await;
    Ok(())
}

/// Downloads `url` to `dest`. A dropped connection (common on the large
/// ffmpeg archives) is retried twice, continuing where it stopped.
async fn fetch(url: &str, dest: &Path, name: &str, progress: &mut impl FnMut(String)) -> Result<()> {
    progress(format!("Lade {name} …"));
    let tmp = dest.with_extension("part");
    let mut part = Part { file: tokio::fs::File::create(&tmp).await?, got: 0, total: 0 };
    let mut attempt = 1;
    loop {
        match fetch_part(url, &mut part, name, progress).await {
            Ok(()) => break,
            // An error answer from the server will not change on retry.
            Err(e) if attempt < 3 && !e.downcast_ref::<reqwest::Error>().is_some_and(|e| e.is_status()) => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(e) => return Err(e),
        }
    }
    let mut file = part.file;
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);
    let _ = tokio::fs::remove_file(dest).await;
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

/// A download in progress: the file written so far and the expected size.
struct Part {
    file: tokio::fs::File,
    got: u64,
    total: u64,
}

/// One attempt of `fetch`, asking only for what is still missing.
async fn fetch_part(url: &str, part: &mut Part, name: &str, progress: &mut impl FnMut(String)) -> Result<()> {
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    let mut request = util::http().get(url);
    if part.got > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={}-", part.got));
    }
    let resp = request
        .send()
        .await
        .with_context(|| format!("{name}: Download fehlgeschlagen"))?
        .error_for_status()
        .with_context(|| format!("{name}: Server antwortete mit Fehler"))?;
    if part.got > 0 && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        // The server sends everything again.
        part.file.set_len(0).await?;
        part.file.seek(std::io::SeekFrom::Start(0)).await?;
        part.got = 0;
    }
    if part.got == 0 {
        part.total = resp.content_length().unwrap_or(0);
    }
    let mut stream = resp.bytes_stream();
    let mut last = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        part.file.write_all(&chunk).await?;
        part.got += chunk.len() as u64;
        if last.elapsed().as_millis() > 200 {
            last = std::time::Instant::now();
            progress(if part.total > 0 {
                format!("Lade {name} … {:.0} %", part.got as f64 / part.total as f64 * 100.0)
            } else {
                format!("Lade {name} … {}", util::fmt_bytes(part.got as f64))
            });
        }
    }
    Ok(())
}

/// Sets the executable bits (Unix). Zips from Windows tools carry none.
fn make_executable(path: &Path) -> std::io::Result<()> {
    set_mode(path, 0o755)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// The yt-dlp zips name the program after the platform (`yt-dlp_linux`);
/// renames it to plain `yt-dlp` so `Tools` finds it everywhere.
fn name_ytdlp(dir: &Path) -> Result<()> {
    let target = dir.join(exe("yt-dlp"));
    if target.is_file() {
        return Ok(());
    }
    let found = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("yt-dlp")))
        .ok_or_else(|| anyhow!("yt-dlp fehlt im Archiv"))?;
    std::fs::rename(found, &target)?;
    Ok(())
}

/// Extracts a zip. `wanted` matches by path suffix and lands flat in `dest`;
/// `None` extracts everything. Unix permissions from the archive are kept.
async fn unzip(zip: &Path, dest: &Path, wanted: Option<Vec<String>>) -> Result<()> {
    let zip = zip.to_path_buf();
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::open(&zip)?;
        let mut archive = zip::ZipArchive::new(file)?;
        std::fs::create_dir_all(&dest)?;
        let names: Vec<String> = archive
            .file_names()
            .map(|n| n.replace('\\', "/"))
            .filter(|n| !n.ends_with('/'))
            .collect();
        let root = common_root(&names);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            if !entry.is_file() {
                continue;
            }
            let name = entry.name().replace('\\', "/");
            let (out, mode) = match &wanted {
                // Picked entries are the programs themselves.
                Some(list) => match list.iter().find(|w| name.ends_with(w.as_str())) {
                    Some(w) => (dest.join(w.rsplit('/').next().unwrap_or(w)), Some(0o755)),
                    None => continue,
                },
                None => {
                    let rel = root.as_deref().and_then(|r| name.strip_prefix(r)).unwrap_or(&name);
                    if rel.is_empty() {
                        continue;
                    }
                    // Refuse path traversal.
                    if rel.split('/').any(|p| p == ".." || p.contains(':')) {
                        return Err(anyhow!("unsicherer Pfad im Archiv: {rel}"));
                    }
                    (dest.join(rel), entry.unix_mode().map(|m| m & 0o777))
                }
            };
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut w = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut w)?;
            if let Some(mode) = mode {
                set_mode(&out, mode)?;
            }
        }
        Ok(())
    })
    .await?
}

/// Extracts the `wanted` programs (matched by path suffix) from a `.tar.xz`
/// flat into `dest`. Stops reading once all are out.
#[cfg(target_os = "linux")]
async fn untar_xz(archive: &Path, dest: &Path, wanted: Vec<String>) -> Result<()> {
    let archive = archive.to_path_buf();
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::io::BufReader::new(std::fs::File::open(&archive)?);
        let mut tar = tar::Archive::new(lzma_rust2::XzReader::new(file, true));
        std::fs::create_dir_all(&dest)?;
        let mut left = wanted;
        for entry in tar.entries()? {
            let mut entry = entry?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let name = entry.path()?.to_string_lossy().replace('\\', "/");
            let Some(i) = left.iter().position(|w| name.ends_with(w.as_str())) else { continue };
            let w = left.swap_remove(i);
            let out = dest.join(w.rsplit('/').next().unwrap_or(&w));
            let mut file = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut file)?;
            make_executable(&out)?;
            if left.is_empty() {
                return Ok(());
            }
        }
        Err(anyhow!("fehlt im Archiv: {}", left.join(", ")))
    })
    .await?
}

#[cfg(not(target_os = "linux"))]
async fn untar_xz(_archive: &Path, _dest: &Path, _wanted: Vec<String>) -> Result<()> {
    Err(anyhow!(".tar.xz wird nur unter Linux entpackt"))
}

/// The folder every entry sits in (`"name/"`), if the archive has exactly one.
/// yt-dlp ships `yt-dlp.exe` next to `_internal/`, which must stay intact.
fn common_root(names: &[String]) -> Option<String> {
    let mut root: Option<&str> = None;
    for name in names {
        let (first, _) = name.split_once('/')?;
        match root {
            None => root = Some(first),
            Some(r) if r == first => {}
            Some(_) => return None,
        }
    }
    root.map(|r| format!("{r}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn temp(sub: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("omnidl-tests").join("deps").join(sub);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn strips_only_a_shared_top_folder() {
        assert_eq!(common_root(&names(&["deno-x/deno.exe", "deno-x/LICENSE"])), Some("deno-x/".into()));
        // yt-dlp_win.zip: the exe sits at top level next to `_internal/`.
        assert_eq!(common_root(&names(&["yt-dlp.exe", "_internal/python310.dll"])), None);
        assert_eq!(common_root(&names(&["a/x", "b/y"])), None);
        assert_eq!(common_root(&names(&[])), None);
    }

    #[tokio::test]
    async fn unzip_keeps_the_ytdlp_layout() {
        use std::io::Write;
        let dir = temp("unzip");
        let zip_path = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.add_directory("_internal/", opts).unwrap();
            w.start_file("_internal/python310.dll", opts).unwrap();
            w.write_all(b"dll").unwrap();
            w.start_file("yt-dlp_linux", opts.unix_permissions(0o755)).unwrap();
            w.write_all(b"exe").unwrap();
            w.finish().unwrap();
        }
        let out = dir.join("out");
        unzip(&zip_path, &out, None).await.unwrap();
        assert!(out.join("_internal").join("python310.dll").is_file());
        name_ytdlp(&out).unwrap();
        let exe_path = out.join(exe("yt-dlp"));
        assert!(exe_path.is_file(), "Programm heißt einheitlich yt-dlp");
        assert!(is_program(&exe_path), "bleibt ausführbar");
    }

    #[tokio::test]
    async fn unzip_picks_programs_flat() {
        use std::io::Write;
        let dir = temp("pick");
        let zip_path = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            for name in ["ffmpeg-x/bin/ffmpeg", "ffmpeg-x/bin/ffprobe", "ffmpeg-x/doc/ffmpeg.html"] {
                w.start_file(name, opts).unwrap();
                w.write_all(b"x").unwrap();
            }
            w.finish().unwrap();
        }
        unzip(&zip_path, &dir, Some(vec!["bin/ffmpeg".into(), "bin/ffprobe".into()])).await.unwrap();
        assert!(is_program(&dir.join("ffmpeg")) && is_program(&dir.join("ffprobe")));
        assert!(!dir.join("ffmpeg.html").exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn untar_xz_picks_programs_flat() {
        let dir = temp("untar");
        let archive = dir.join("t.tar.xz");
        {
            let xz = lzma_rust2::XzWriter::new(std::fs::File::create(&archive).unwrap(), Default::default()).unwrap();
            let mut tar = tar::Builder::new(xz);
            for name in ["ff/doc/ffmpeg.html", "ff/bin/ffmpeg", "ff/bin/ffplay", "ff/bin/ffprobe"] {
                let mut header = tar::Header::new_gnu();
                header.set_size(1);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, name, &b"x"[..]).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        untar_xz(&archive, &dir, vec!["bin/ffmpeg".into(), "bin/ffprobe".into()]).await.unwrap();
        assert!(is_program(&dir.join("ffmpeg")) && is_program(&dir.join("ffprobe")));
        assert!(!dir.join("ffplay").exists() && !dir.join("ffmpeg.html").exists());
        let err = untar_xz(&archive, &dir, vec!["bin/fehlt".into()]).await.unwrap_err();
        assert!(err.to_string().contains("bin/fehlt"));
    }

    /// Bricht die Verbindung mitten im Download ab, setzt der zweite Versuch
    /// dort fort, wo der erste aufgehört hat.
    #[tokio::test]
    async fn fetch_resumes_a_dropped_download() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/tool.zip", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ranges = Vec::new();
            for answer in [
                "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123",
                "HTTP/1.1 206 Partial Content\r\nContent-Length: 6\r\nContent-Range: bytes 4-9/10\r\n\r\n456789",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0u8; 4096];
                let n = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..n]).to_lowercase();
                ranges.push(request.lines().find(|l| l.starts_with("range:")).map(str::to_string));
                socket.write_all(answer.as_bytes()).await.unwrap();
                // Dropping the socket ends the first answer six bytes short.
            }
            ranges
        });
        let dest = temp("resume").join("tool.zip");
        let mut messages = Vec::new();
        fetch(&url, &dest, "tool", &mut |m| messages.push(m)).await.expect("zweiter Versuch klappt");
        assert_eq!(std::fs::read(&dest).unwrap(), b"0123456789");
        assert_eq!(server.await.unwrap(), vec![None, Some("range: bytes=4-".to_string())]);
    }

    #[test]
    fn parses_ytdlp_version_age() {
        let today = util::today_days();
        let d = util::days_from_civil(2026, 9, 1);
        assert_eq!(version_age_days("2026.09.01"), Some(today - d));
        assert_eq!(version_age_days("2026.08.19.123456"), Some(today - util::days_from_civil(2026, 8, 19)));
        assert_eq!(version_age_days("kaputt"), None);
    }

    /// Every platform with ready-made builds, as (os, arch, musl).
    const PLATFORMS: [(&str, &str, bool); 8] = [
        ("windows", "x86_64", false),
        ("windows", "aarch64", false),
        ("macos", "x86_64", false),
        ("macos", "aarch64", false),
        ("linux", "x86_64", false),
        ("linux", "aarch64", false),
        ("linux", "x86_64", true),
        ("linux", "aarch64", true),
    ];

    #[test]
    fn windows_downloads_stay_the_same() {
        let s = Sources::of("windows", "x86_64", false);
        assert_eq!(s.ytdlp.as_deref(), Some("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_win.zip"));
        assert_eq!(
            s.ffmpeg,
            Some(Ffmpeg::Bundle(
                "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip".into()
            ))
        );
        assert_eq!(
            s.deno.as_deref(),
            Some("https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip")
        );
        assert_eq!(s.gallery.as_deref(), Some("https://codeberg.org/mikf/gallery-dl/releases/download/latest/gallery-dl.exe"));
    }

    #[test]
    fn every_platform_gets_its_own_builds() {
        for (os, arch, musl) in PLATFORMS {
            let s = Sources::of(os, arch, musl);
            assert!(s.ytdlp.is_some() && s.ffmpeg.is_some(), "{os}/{arch}: yt-dlp und ffmpeg gibt es überall");
        }
        let linux_arm = Sources::of("linux", "aarch64", false);
        assert!(linux_arm.ytdlp.unwrap().ends_with("/yt-dlp_linux_aarch64.zip"));
        assert!(matches!(linux_arm.ffmpeg, Some(Ffmpeg::Bundle(u)) if u.ends_with("-linuxarm64-gpl.tar.xz")));
        assert!(linux_arm.deno.unwrap().ends_with("/deno-aarch64-unknown-linux-gnu.zip"));

        let linux = Sources::of("linux", "x86_64", false);
        assert!(linux.ytdlp.unwrap().ends_with("/yt-dlp_linux.zip"));
        assert!(linux.gallery.unwrap().ends_with("/gallery-dl.bin"));
        assert!(Sources::of("linux", "x86_64", true).ytdlp.unwrap().ends_with("/yt-dlp_musllinux.zip"));

        let mac = Sources::of("macos", "aarch64", false);
        assert!(mac.ytdlp.unwrap().ends_with("/yt-dlp_macos.zip"));
        assert!(matches!(mac.ffmpeg, Some(Ffmpeg::Pair { ffmpeg, ffprobe })
            if ffmpeg.contains("/macos/arm64/") && ffprobe.ends_with("/ffprobe.zip")));
        assert!(mac.deno.unwrap().ends_with("/deno-aarch64-apple-darwin.zip"));
        assert!(mac.gallery.is_none(), "gallery-dl gibt es nicht für macOS");
        assert!(matches!(Sources::of("macos", "x86_64", false).ffmpeg, Some(Ffmpeg::Pair { ffmpeg, .. })
            if ffmpeg.contains("/macos/amd64/")));

        assert_eq!(Sources::of("freebsd", "x86_64", false), Sources::default(), "unbekannte Systeme laden nichts");
        assert_eq!(Sources::of("linux", "arm", false), Sources::default());
    }

    #[test]
    fn this_platform_is_supported() {
        assert!(Sources::current().ytdlp.is_some(), "Build-Ziel ohne Downloads");
    }

    #[test]
    fn installed_programs_need_the_executable_bit() {
        let dir = temp("path");
        std::fs::write(dir.join(exe("omnidl-tool")), "x").unwrap();
        make_executable(&dir.join(exe("omnidl-tool"))).unwrap();
        std::fs::write(dir.join(exe("omnidl-data")), "x").unwrap();
        let dirs = [dir.join("fehlt"), dir.clone()];
        assert_eq!(find_in(&dirs, "omnidl-tool"), Some(dir.join(exe("omnidl-tool"))));
        assert_eq!(find_in(&dirs, "omnidl-nichts"), None);
        #[cfg(unix)]
        assert_eq!(find_in(&dirs, "omnidl-data"), None, "ohne x-Bit kein Programm");
    }

    #[test]
    fn downloaded_tools_win_over_installed_ones() {
        let base = temp("pick-base");
        let tools = Tools::new(&base);
        std::fs::create_dir_all(&tools.bin).unwrap();
        let deno = tools.bin.join(exe("deno"));
        std::fs::write(&deno, "x").unwrap();
        make_executable(&deno).unwrap();
        assert_eq!(tools.deno(), deno);
        assert_eq!(tools.js_runtime(), Some(format!("deno:{}", deno.display())));
        for name in ["ffmpeg", "ffprobe"] {
            std::fs::write(tools.bin.join(exe(name)), "x").unwrap();
        }
        assert_eq!(tools.ffmpeg_dir(), tools.bin);
        assert_eq!(tools.ffprobe(), tools.bin.join(exe("ffprobe")));
    }

    /// Prüft, dass jeder Download-Link aller Plattformen noch antwortet.
    #[tokio::test]
    #[ignore = "braucht Netzwerk"]
    async fn every_download_link_answers() {
        let mut urls = Vec::new();
        for (os, arch, musl) in PLATFORMS {
            let s = Sources::of(os, arch, musl);
            urls.extend(s.ytdlp.into_iter().chain(s.deno).chain(s.gallery));
            match s.ffmpeg {
                Some(Ffmpeg::Bundle(u)) => urls.push(u),
                Some(Ffmpeg::Pair { ffmpeg, ffprobe }) => urls.extend([ffmpeg, ffprobe]),
                None => {}
            }
        }
        urls.sort();
        urls.dedup();
        // Own client: pooled connections of the shared one would die with this
        // test's runtime and break tests running beside it.
        let http = reqwest::Client::builder().user_agent(util::USER_AGENT).build().unwrap();
        for url in urls {
            let resp = http.get(&url).header("Range", "bytes=0-0").send().await;
            let status = resp.map(|r| r.status());
            assert!(status.as_ref().is_ok_and(|s| s.is_success()), "{url}: {status:?}");
        }
    }

    /// Richtet die Werkzeuge dieses Systems in einem leeren Ordner ein und
    /// startet sie. Unter macOS/Linux der Beweis, dass Download und Entpacken
    /// dort funktionieren.
    #[tokio::test]
    #[ignore = "braucht Netzwerk"]
    async fn e2e_ensure_installs_working_tools() {
        let base = std::env::temp_dir().join("omnidl-tests").join("deps-e2e");
        let _ = std::fs::remove_dir_all(&base);
        let tools = Tools::new(&base);
        let mut messages = Vec::new();
        ensure(&tools, true, |m| messages.push(m)).await.expect("Werkzeuge eingerichtet");
        let s = status(&tools).await;
        eprintln!("{s:?}\nyt-dlp: {}\nffmpeg: {}", tools.ytdlp().display(), tools.ffmpeg().display());
        assert!(s.ytdlp.is_some(), "yt-dlp startet");
        assert!(tools.ytdlp().starts_with(&tools.bin), "frisch geladenes yt-dlp");
        assert!(s.ffmpeg, "ffmpeg und ffprobe da");
        assert!(s.js.is_some(), "JavaScript-Laufzeit da");
        assert!(s.ready());
        assert!(messages.iter().any(|m| m.starts_with("Lade yt-dlp")));
        let out = util::command(&tools.ffmpeg()).arg("-version").output().await.expect("ffmpeg startet");
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("ffmpeg version"));
        let out = util::command(&tools.ffprobe()).arg("-version").output().await.expect("ffprobe startet");
        assert!(out.status.success());
        // Where Node is installed (CI runners) ensure skips Deno; load it anyway.
        if let Some(url) = &Sources::current().deno {
            let deno = tools.bin.join(exe("deno"));
            if !deno.is_file() {
                install_from_zip(&tools, url, "deno", &mut |_| {}).await.expect("Deno geladen");
            }
            let out = util::command(&deno).arg("--version").output().await.expect("deno startet");
            assert!(String::from_utf8_lossy(&out.stdout).starts_with("deno "));
        }
        if Sources::current().gallery.is_some() {
            let out = util::command(&tools.gallery_dl()).arg("--version").output().await.expect("gallery-dl startet");
            assert!(out.status.success());
        }
    }
}
