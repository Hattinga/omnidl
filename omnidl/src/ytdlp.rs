use crate::config::{Config, Format};
use crate::deps::Tools;
use crate::engine::Shared;
use crate::job::{JobId, JobState, JobUpdate};
use crate::util;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

/// Marker prefixes we make yt-dlp print so its output can be parsed reliably.
const P_PROGRESS: &str = "OMNI ";
const P_TITLE: &str = "OMNITITLE ";
const P_FILE: &str = "OMNIFILE ";

pub struct Request<'a> {
    pub url: &'a str,
    pub dir: &'a Path,
    /// yt-dlp output template, relative to `dir`.
    pub template: String,
    pub cfg: &'a Config,
    pub playlist: bool,
    pub embed_meta: bool,
    /// Forces audio extraction even if the user picked a video format (Spotify).
    pub force_audio: bool,
    /// Accept only media whose duration is within these seconds.
    pub duration: Option<(f64, f64)>,
}

#[derive(Default, Debug)]
pub struct Outcome {
    pub files: Vec<PathBuf>,
    /// Media was skipped because it did not pass `--match-filter`.
    pub filtered: bool,
    pub error: Option<String>,
    pub cancelled: bool,
}

/// The format actually used, honouring `force_audio`.
pub fn effective_format(cfg: &Config, force_audio: bool) -> Format {
    if force_audio && !cfg.format.is_audio() { Format::Mp3 } else { cfg.format }
}

pub fn build_args(tools: &Tools, req: &Request) -> Vec<String> {
    let cfg = req.cfg;
    let mut a: Vec<String> = vec![
        "--newline".into(),
        "--no-quiet".into(),
        "--progress".into(),
        "--color".into(),
        "never".into(),
        "--no-mtime".into(),
        "--windows-filenames".into(),
        "--no-warnings".into(),
        "-N".into(),
        "8".into(),
        "--retries".into(),
        "10".into(),
        "--fragment-retries".into(),
        "10".into(),
        "--ffmpeg-location".into(),
        tools.ffmpeg_dir().display().to_string(),
        "--progress-template".into(),
        format!(
            "download:{P_PROGRESS}%(progress.downloaded_bytes)s %(progress.total_bytes)s \
             %(progress.total_bytes_estimate)s %(progress.speed)s %(progress.eta)s"
        ),
        "--print".into(),
        format!("before_dl:{P_TITLE}%(title)s"),
        "--print".into(),
        format!("after_move:{P_FILE}%(filepath)s"),
        "-P".into(),
        req.dir.display().to_string(),
        "-o".into(),
        req.template.clone(),
    ];

    if let Some(js) = tools.js_runtime() {
        a.push("--js-runtimes".into());
        a.push(js);
    }
    if let Some(browser) = cfg.cookies.browser() {
        a.push("--cookies-from-browser".into());
        a.push(browser.into());
    }
    a.push(if req.playlist { "--yes-playlist".into() } else { "--no-playlist".into() });

    if let Some((lo, hi)) = req.duration {
        a.push("--match-filter".into());
        a.push(format!("duration>={lo:.0} & duration<={hi:.0}"));
    }

    let fmt = effective_format(cfg, req.force_audio);
    if fmt.is_audio() {
        a.push("-f".into());
        a.push("ba/b".into());
        a.push("-x".into());
        a.push("--audio-format".into());
        a.push(fmt.ext().into());
        if fmt.has_bitrate() {
            a.push("--audio-quality".into());
            a.push(cfg.audio_quality.ytdlp_value().into());
        }
    } else {
        a.push("-f".into());
        a.push("bv*+ba/b".into());
        a.push("-S".into());
        a.push(match (cfg.video_quality.height(), fmt) {
            // Prefer H.264/AAC in MP4 so anything can play the result.
            (Some(h), Format::Mp4) => format!("res:{h},vcodec:h264,acodec:aac"),
            (None, Format::Mp4) => "res,vcodec:h264,acodec:aac".into(),
            (Some(h), _) => format!("res:{h}"),
            (None, _) => "res".into(),
        });
        a.push("--merge-output-format".into());
        a.push(fmt.ext().into());
    }

    if req.embed_meta {
        a.push("--embed-metadata".into());
        if !matches!(fmt, Format::Wav) {
            a.push("--embed-thumbnail".into());
        }
        if !fmt.is_audio() {
            a.push("--embed-chapters".into());
        }
    }

    a.push("--".into());
    a.push(req.url.into());
    a
}

/// Parsed `OMNI` progress line: (fraction, bytes/s, eta seconds).
pub fn parse_progress(line: &str) -> Option<(Option<f32>, Option<f64>, Option<u64>)> {
    let rest = line.strip_prefix(P_PROGRESS)?;
    let num = |s: &str| -> Option<f64> {
        if s == "NA" || s.is_empty() { None } else { s.parse::<f64>().ok() }
    };
    let mut it = rest.split_whitespace();
    let done = num(it.next()?);
    let total = num(it.next()?);
    let estimate = num(it.next()?);
    let speed = num(it.next().unwrap_or("NA"));
    let eta = num(it.next().unwrap_or("NA"));
    let denom = total.or(estimate);
    let frac = match (done, denom) {
        (Some(d), Some(t)) if t > 0.0 => Some((d / t).clamp(0.0, 1.0) as f32),
        _ => None,
    };
    Some((frac, speed, eta.map(|e| e as u64)))
}

/// `[download] Downloading item 3 of 20`
fn parse_item(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("[download] Downloading item ")?;
    let (i, rest) = rest.split_once(" of ")?;
    Some((i.trim().parse().ok()?, rest.trim().parse().ok()?))
}

fn is_processing(line: &str) -> bool {
    const TAGS: [&str; 7] = [
        "[Merger]",
        "[ExtractAudio]",
        "[EmbedThumbnail]",
        "[Metadata]",
        "[VideoConvertor]",
        "[VideoRemuxer]",
        "[FixupM3u8]",
    ];
    TAGS.iter().any(|t| line.starts_with(t))
}

enum Line {
    Out(String),
    Err(String),
}

pub async fn run(
    shared: &Shared,
    id: JobId,
    tools: &Tools,
    req: &Request<'_>,
    token: &CancellationToken,
) -> Outcome {
    let mut out = Outcome::default();
    if let Err(e) = tokio::fs::create_dir_all(req.dir).await {
        out.error = Some(format!("Zielordner nicht anlegbar: {e}"));
        return out;
    }

    let args = build_args(tools, req);
    let mut child = match util::command(&tools.ytdlp())
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            out.error = Some(format!("yt-dlp nicht startbar: {e}"));
            return out;
        }
    };
    let pid = child.id();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Line>();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                if tx.send(Line::Out(l)).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                if tx.send(Line::Err(l)).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);

    let mut last_progress = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let mut last_error: Option<String> = None;

    loop {
        tokio::select! {
            _ = token.cancelled() => {
                if let Some(pid) = pid { util::kill_tree(pid); }
                let _ = child.wait().await;
                out.cancelled = true;
                return out;
            }
            line = rx.recv() => {
                let Some(line) = line else { break };
                match line {
                    Line::Out(l) => {
                        if let Some(title) = l.strip_prefix(P_TITLE) {
                            shared.emit(id, JobUpdate::Title(title.trim().to_string()));
                        } else if let Some(path) = l.strip_prefix(P_FILE) {
                            out.files.push(PathBuf::from(path.trim()));
                        } else if let Some((frac, speed, eta)) = parse_progress(&l) {
                            if last_progress.elapsed().as_millis() >= 100 || frac.is_some_and(|f| f >= 1.0) {
                                last_progress = std::time::Instant::now();
                                shared.emit(id, JobUpdate::Progress { frac, speed, eta });
                            }
                        } else if let Some((i, n)) = parse_item(&l) {
                            shared.emit(id, JobUpdate::Item { index: i, count: n });
                        } else if is_processing(&l) {
                            shared.emit(id, JobUpdate::State(JobState::Processing));
                        } else if l.contains("does not pass filter") {
                            out.filtered = true;
                        } else if l.starts_with("[download] Destination:") {
                            shared.emit(id, JobUpdate::State(JobState::Downloading));
                        }
                    }
                    Line::Err(l) => {
                        if let Some(msg) = l.strip_prefix("ERROR: ") {
                            last_error = Some(msg.trim().to_string());
                        }
                    }
                }
            }
        }
    }

    let status = child.wait().await;
    let ok = matches!(&status, Ok(s) if s.success());
    if out.files.is_empty() && !out.filtered {
        out.error = Some(match (ok, last_error) {
            (_, Some(e)) => e,
            (false, None) => "yt-dlp ist fehlgeschlagen".to_string(),
            (true, None) => "keine Datei erzeugt".to_string(),
        });
    }
    out
}

/// Flat playlist/search extraction: returns (list title, entries).
pub struct Entry {
    pub url: String,
    pub title: String,
    pub duration: Option<f64>,
    pub channel: Option<String>,
}

pub async fn flat_list(
    tools: &Tools,
    target: &str,
    limit: Option<u32>,
    cookies: Option<&str>,
) -> anyhow::Result<(Option<String>, Vec<Entry>)> {
    let mut cmd = util::command(&tools.ytdlp());
    cmd.args(["--flat-playlist", "-J", "--no-warnings", "--color", "never"]);
    if let Some(js) = tools.js_runtime() {
        cmd.args(["--js-runtimes", &js]);
    }
    if let Some(b) = cookies {
        cmd.args(["--cookies-from-browser", b]);
    }
    if let Some(n) = limit {
        cmd.args(["--playlist-end", &n.to_string()]);
    }
    cmd.arg("--").arg(target);
    let out = cmd.output().await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err
            .lines()
            .find_map(|l| l.strip_prefix("ERROR: "))
            .unwrap_or("Liste konnte nicht gelesen werden");
        anyhow::bail!("{}", msg.trim());
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let title = json.get("title").and_then(|t| t.as_str()).map(|s| s.to_string());
    let entries = json
        .get("entries")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let url = e
                        .get("url")
                        .and_then(|u| u.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            e.get("id")
                                .and_then(|i| i.as_str())
                                .map(|id| format!("https://www.youtube.com/watch?v={id}"))
                        })?;
                    Some(Entry {
                        url,
                        title: e
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("(ohne Titel)")
                            .to_string(),
                        duration: e.get("duration").and_then(|d| d.as_f64()),
                        channel: e
                            .get("channel")
                            .or_else(|| e.get("uploader"))
                            .and_then(|c| c.as_str())
                            .map(|s| s.to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((title, entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AudioQuality, Cookies, Format, VideoQuality};

    fn cfg(format: Format) -> Config {
        Config {
            download_dir: PathBuf::from("C:/dl"),
            parallel: 4,
            format,
            video_quality: VideoQuality::P1080,
            audio_quality: AudioQuality::K320,
            playlist: false,
            cookies: Cookies::Off,
            ..Config::default()
        }
    }

    #[test]
    fn progress_lines() {
        let (f, s, e) = parse_progress("OMNI 1024 2048 NA 38605.4 5").unwrap();
        assert_eq!(f, Some(0.5));
        assert_eq!(s, Some(38605.4));
        assert_eq!(e, Some(5));
        // total unknown -> estimate is used
        let (f, _, _) = parse_progress("OMNI 50 NA 200 NA NA").unwrap();
        assert_eq!(f, Some(0.25));
        // nothing known at all
        let (f, s, e) = parse_progress("OMNI NA NA NA NA NA").unwrap();
        assert!(f.is_none() && s.is_none() && e.is_none());
        assert!(parse_progress("[download] Destination: x").is_none());
    }

    #[test]
    fn item_lines() {
        assert_eq!(parse_item("[download] Downloading item 3 of 20"), Some((3, 20)));
        assert_eq!(parse_item("[download] Downloading item x of y"), None);
        assert_eq!(parse_item("irgendwas"), None);
    }

    #[test]
    fn audio_args_extract_and_convert() {
        let tools = Tools::new(Path::new("C:/app"));
        let c = cfg(Format::Mp3);
        let req = Request {
            url: "https://x/1",
            dir: Path::new("C:/dl"),
            template: "%(title)s.%(ext)s".into(),
            cfg: &c,
            playlist: false,
            embed_meta: true,
            force_audio: false,
            duration: Some((180.0, 200.0)),
        };
        let a = build_args(&tools, &req);
        let joined = a.join(" ");
        assert!(joined.contains("-x --audio-format mp3 --audio-quality 320K"));
        assert!(joined.contains("--match-filter duration>=180 & duration<=200"));
        assert!(joined.contains("--embed-thumbnail"));
        assert!(joined.ends_with("-- https://x/1"));
        assert!(joined.contains("--no-playlist"));
    }

    #[test]
    fn video_args_prefer_compatible_codecs() {
        let tools = Tools::new(Path::new("C:/app"));
        let c = cfg(Format::Mp4);
        let req = Request {
            url: "https://x/1",
            dir: Path::new("C:/dl"),
            template: "%(title)s.%(ext)s".into(),
            cfg: &c,
            playlist: true,
            embed_meta: false,
            force_audio: false,
            duration: None,
        };
        let joined = build_args(&tools, &req).join(" ");
        assert!(joined.contains("-S res:1080,vcodec:h264,acodec:aac"));
        assert!(joined.contains("--merge-output-format mp4"));
        assert!(joined.contains("--yes-playlist"));
        assert!(!joined.contains("--embed-metadata"));
    }

    #[test]
    fn force_audio_overrides_video_format() {
        let c = cfg(Format::Mp4);
        assert_eq!(effective_format(&c, true), Format::Mp3);
        assert_eq!(effective_format(&c, false), Format::Mp4);
        let c = cfg(Format::Flac);
        assert_eq!(effective_format(&c, true), Format::Flac);
    }
}
