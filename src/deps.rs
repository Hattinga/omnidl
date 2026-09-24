use crate::util;
use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};

/// External binaries the app drives. All live in `<base>/bin`.
pub struct Tools {
    pub bin: PathBuf,
}

impl Tools {
    pub fn new(base: &Path) -> Self {
        Self { bin: base.join("bin") }
    }

    pub fn ytdlp(&self) -> PathBuf {
        self.bin.join("yt-dlp").join("yt-dlp.exe")
    }
    pub fn ffmpeg(&self) -> PathBuf {
        self.bin.join("ffmpeg.exe")
    }
    pub fn ffprobe(&self) -> PathBuf {
        self.bin.join("ffprobe.exe")
    }
    pub fn deno(&self) -> PathBuf {
        self.bin.join("deno.exe")
    }
    pub fn gallery_dl(&self) -> PathBuf {
        self.bin.join("gallery-dl.exe")
    }

    /// Value for yt-dlp `--js-runtimes`. YouTube needs one since late 2025.
    pub fn js_runtime(&self) -> Option<String> {
        if self.deno().is_file() {
            return Some(format!("deno:{}", self.deno().display()));
        }
        if which_node().is_some() {
            return Some("node".to_string());
        }
        None
    }
}

fn which_node() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|p| p.join("node.exe"))
        .find(|p| p.is_file())
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

const YTDLP_ZIP: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_win.zip";
const FFMPEG_ZIP: &str =
    "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
const DENO_ZIP: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
const GALLERY_EXE: &str = "https://codeberg.org/mikf/gallery-dl/releases/download/latest/gallery-dl.exe";

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
    if let Some(v) = ytdlp_version(tools).await {
        // A fresh download of the newest release is not stale, however old the release is.
        let refreshed = days_since_modified(&tools.ytdlp());
        s.ytdlp_age_days = version_age_days(&v).map(|age| refreshed.map_or(age, |d| age.min(d)));
        s.ytdlp = Some(v);
    }
    s
}

/// Days since the file was written, i.e. since the last install or update.
fn days_since_modified(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(modified).ok()?;
    Some((age.as_secs() / 86_400) as i64)
}

/// `None` if yt-dlp is missing or does not start (e.g. a damaged install).
async fn ytdlp_version(tools: &Tools) -> Option<String> {
    if !tools.ytdlp().is_file() {
        return None;
    }
    let out = util::command(&tools.ytdlp()).arg("--version").output().await.ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

/// yt-dlp versions are dates: `2026.08.19`.
fn version_age_days(v: &str) -> Option<i64> {
    let mut it = v.split('.');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    Some(util::today_days() - util::days_from_civil(y, m, d))
}

/// Downloads whatever is missing. `progress` reports a human readable status line.
pub async fn ensure(tools: &Tools, force_ytdlp: bool, mut progress: impl FnMut(String)) -> Result<()> {
    tokio::fs::create_dir_all(&tools.bin).await.ok();

    if force_ytdlp || ytdlp_version(tools).await.is_none() {
        let zip = tools.bin.join("yt-dlp.zip");
        fetch(YTDLP_ZIP, &zip, "yt-dlp", &mut progress).await?;
        let dest = tools.bin.join("yt-dlp");
        let tmp = tools.bin.join("yt-dlp.new");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        unzip(&zip, &tmp, None).await?;
        let _ = tokio::fs::remove_dir_all(&dest).await;
        tokio::fs::rename(&tmp, &dest)
            .await
            .context("yt-dlp konnte nicht ersetzt werden (läuft noch ein Download?)")?;
        let _ = tokio::fs::remove_file(&zip).await;
    }

    if !tools.ffmpeg().is_file() || !tools.ffprobe().is_file() {
        let zip = tools.bin.join("ffmpeg.zip");
        fetch(FFMPEG_ZIP, &zip, "ffmpeg", &mut progress).await?;
        unzip(&zip, &tools.bin, Some(&["bin/ffmpeg.exe", "bin/ffprobe.exe"])).await?;
        let _ = tokio::fs::remove_file(&zip).await;
    }

    if !tools.deno().is_file() && which_node().is_none() {
        let zip = tools.bin.join("deno.zip");
        fetch(DENO_ZIP, &zip, "deno", &mut progress).await?;
        unzip(&zip, &tools.bin, Some(&["deno.exe"])).await?;
        let _ = tokio::fs::remove_file(&zip).await;
    }

    if !tools.gallery_dl().is_file() {
        fetch(GALLERY_EXE, &tools.gallery_dl(), "gallery-dl", &mut progress).await?;
    }
    Ok(())
}

async fn fetch(url: &str, dest: &Path, name: &str, progress: &mut impl FnMut(String)) -> Result<()> {
    progress(format!("Lade {name} …"));
    let resp = util::http()
        .get(url)
        .send()
        .await
        .with_context(|| format!("{name}: Download fehlgeschlagen"))?
        .error_for_status()
        .with_context(|| format!("{name}: Server antwortete mit Fehler"))?;
    let total = resp.content_length().unwrap_or(0);
    let tmp = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut stream = resp.bytes_stream();
    let mut got: u64 = 0;
    let mut last = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        got += chunk.len() as u64;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        if last.elapsed().as_millis() > 200 {
            last = std::time::Instant::now();
            progress(if total > 0 {
                format!("Lade {name} … {:.0} %", got as f64 / total as f64 * 100.0)
            } else {
                format!("Lade {name} … {}", util::fmt_bytes(got as f64))
            });
        }
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);
    let _ = tokio::fs::remove_file(dest).await;
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

/// Extracts a zip. `wanted` matches by path suffix; `None` extracts everything.
async fn unzip(zip: &Path, dest: &Path, wanted: Option<&'static [&'static str]>) -> Result<()> {
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
            let out = match wanted {
                Some(list) => match list.iter().find(|w| name.ends_with(*w)) {
                    Some(w) => dest.join(w.rsplit('/').next().unwrap_or(w)),
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
                    dest.join(rel)
                }
            };
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut w = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut w)?;
        }
        Ok(())
    })
    .await?
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
        let dir = std::env::temp_dir().join("omnidl-tests").join("unzip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.add_directory("_internal/", opts).unwrap();
            w.start_file("_internal/python310.dll", opts).unwrap();
            w.write_all(b"dll").unwrap();
            w.start_file("yt-dlp.exe", opts).unwrap();
            w.write_all(b"exe").unwrap();
            w.finish().unwrap();
        }
        let out = dir.join("out");
        unzip(&zip_path, &out, None).await.unwrap();
        assert!(out.join("yt-dlp.exe").is_file());
        assert!(out.join("_internal").join("python310.dll").is_file());
    }

    #[test]
    fn parses_ytdlp_version_age() {
        let today = util::today_days();
        let d = util::days_from_civil(2026, 9, 1);
        assert_eq!(version_age_days("2026.09.01"), Some(today - d));
        assert_eq!(version_age_days("2026.08.19.123456"), Some(today - util::days_from_civil(2026, 8, 19)));
        assert_eq!(version_age_days("kaputt"), None);
    }
}
