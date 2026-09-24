//! `omnidl-cli get`: download links from the terminal.

use crate::term::{self, Live, Paint};
use crate::tools::STALE_DAYS;
use clap::ValueEnum;
use omnidl::config::{self, AudioQuality, Config, Cookies, Format, VideoQuality};
use omnidl::deps::DepsStatus;
use omnidl::engine::{self, Command};
use omnidl::job::{Event, JobId, JobState, JobUpdate};
use omnidl::jobs::{Job, Jobs};
use omnidl::{detect, launch, schedule, sys};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Wrong arguments; the same code clap uses.
const USAGE: u8 = 2;
/// After Ctrl+C (128 + SIGINT), as shells expect.
pub const INTERRUPTED: u8 = 130;
/// How long cancelled jobs get to report back before the program ends anyway.
const GRACE: Duration = Duration::from_secs(3);
/// Plain output repeats the progress of a running job this often.
const PLAIN_PROGRESS: Duration = Duration::from_secs(10);
/// JSON output passes progress on at most this often per job.
const JSON_PROGRESS: Duration = Duration::from_secs(1);

#[derive(clap::Args, Debug)]
#[command(after_help = "Ohne Optionen gelten die Einstellungen der App. Die Optionen gelten nur für diesen Aufruf.")]
pub struct Args {
    /// Links (YouTube, TikTok, Spotify, …), auch mehrere.
    #[arg(required = true, value_name = "LINK")]
    pub urls: Vec<String>,

    /// Nur Audio, im zuletzt gewählten Audioformat.
    #[arg(short, long, conflicts_with = "format")]
    pub audio: bool,

    /// Dateiformat.
    #[arg(short, long, value_enum)]
    pub format: Option<FormatArg>,

    /// Höchste Videoauflösung.
    #[arg(short, long, value_enum, value_name = "AUFLÖSUNG")]
    pub quality: Option<QualityArg>,

    /// Audio-Bitrate in kbit/s.
    #[arg(long, value_enum, value_name = "KBITS")]
    pub bitrate: Option<BitrateArg>,

    /// Zielordner.
    #[arg(short = 'o', long, value_name = "ORDNER")]
    pub dir: Option<PathBuf>,

    /// Gleichzeitige Downloads (1–16).
    #[arg(short = 'j', long, value_name = "ANZAHL", value_parser = clap::value_parser!(u8).range(1..=16))]
    pub parallel: Option<u8>,

    /// Bei Links mit Playlist nur das eine Video laden.
    #[arg(long)]
    pub no_playlist: bool,

    /// Anmeldung (Cookies) aus diesem Browser verwenden.
    #[arg(long, value_enum, value_name = "BROWSER")]
    pub cookies: Option<CookiesArg>,

    /// Erst um diese Uhrzeit starten und bis dahin warten.
    #[arg(long, value_name = "HH:MM", value_parser = parse_at)]
    pub at: Option<(u32, u32)>,

    /// Ein JSON-Objekt pro Ereignis und Zeile, für Skripte.
    #[arg(long, conflicts_with = "quiet")]
    pub json: bool,

    /// Keine Fortschrittsanzeige; gibt nur die fertigen Dateien aus.
    #[arg(long)]
    pub quiet: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FormatArg {
    Mp4,
    Mkv,
    Mp3,
    M4a,
    Opus,
    Flac,
    Wav,
}

impl From<FormatArg> for Format {
    fn from(f: FormatArg) -> Self {
        match f {
            FormatArg::Mp4 => Format::Mp4,
            FormatArg::Mkv => Format::Mkv,
            FormatArg::Mp3 => Format::Mp3,
            FormatArg::M4a => Format::M4a,
            FormatArg::Opus => Format::Opus,
            FormatArg::Flac => Format::Flac,
            FormatArg::Wav => Format::Wav,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum QualityArg {
    Best,
    #[value(name = "2160")]
    P2160,
    #[value(name = "1440")]
    P1440,
    #[value(name = "1080")]
    P1080,
    #[value(name = "720")]
    P720,
    #[value(name = "480")]
    P480,
    #[value(name = "360")]
    P360,
}

impl From<QualityArg> for VideoQuality {
    fn from(q: QualityArg) -> Self {
        match q {
            QualityArg::Best => VideoQuality::Best,
            QualityArg::P2160 => VideoQuality::P2160,
            QualityArg::P1440 => VideoQuality::P1440,
            QualityArg::P1080 => VideoQuality::P1080,
            QualityArg::P720 => VideoQuality::P720,
            QualityArg::P480 => VideoQuality::P480,
            QualityArg::P360 => VideoQuality::P360,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum BitrateArg {
    Best,
    #[value(name = "320")]
    K320,
    #[value(name = "256")]
    K256,
    #[value(name = "192")]
    K192,
    #[value(name = "128")]
    K128,
}

impl From<BitrateArg> for AudioQuality {
    fn from(b: BitrateArg) -> Self {
        match b {
            BitrateArg::Best => AudioQuality::Best,
            BitrateArg::K320 => AudioQuality::K320,
            BitrateArg::K256 => AudioQuality::K256,
            BitrateArg::K192 => AudioQuality::K192,
            BitrateArg::K128 => AudioQuality::K128,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CookiesArg {
    Firefox,
    Chrome,
    Edge,
    Brave,
    /// Keine Cookies, auch wenn die App welche nutzt.
    Off,
}

impl From<CookiesArg> for Cookies {
    fn from(c: CookiesArg) -> Self {
        match c {
            CookiesArg::Firefox => Cookies::Firefox,
            CookiesArg::Chrome => Cookies::Chrome,
            CookiesArg::Edge => Cookies::Edge,
            CookiesArg::Brave => Cookies::Brave,
            CookiesArg::Off => Cookies::Off,
        }
    }
}

/// "2:30" or "02:30" as (hour, minute).
fn parse_at(s: &str) -> Result<(u32, u32), String> {
    let digits = |p: &str, len: std::ops::RangeInclusive<usize>| {
        (len.contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse::<u32>().ok()).flatten()
    };
    s.trim()
        .split_once(':')
        .and_then(|(h, m)| Some((digits(h, 1..=2)?, digits(m, 2..=2)?)))
        .filter(|&(h, m)| h < 24 && m < 60)
        .ok_or_else(|| format!("„{s}“ ist keine Uhrzeit, bitte als HH:MM angeben, etwa 02:30"))
}

/// The settings for this run: the app's, with the flags on top. Nothing is saved.
fn config(args: &Args, mut cfg: Config) -> Config {
    if args.audio {
        cfg.set_audio(true);
    }
    if let Some(f) = args.format {
        cfg.set_format(f.into());
    }
    if let Some(q) = args.quality {
        cfg.video_quality = q.into();
    }
    if let Some(b) = args.bitrate {
        cfg.audio_quality = b.into();
    }
    if let Some(dir) = &args.dir {
        // yt-dlp reports where files went relative to this; make it absolute.
        cfg.download_dir = std::path::absolute(dir).unwrap_or_else(|_| dir.clone());
    }
    if let Some(n) = args.parallel {
        cfg.parallel = n.into();
    }
    if args.no_playlist {
        cfg.playlist = false;
    }
    if let Some(c) = args.cookies {
        cfg.cookies = c.into();
    }
    cfg
}

/// Every argument must hold at least one web link; the ones that don't are returned.
fn links(args: &[String]) -> Result<Vec<String>, Vec<String>> {
    let mut urls = Vec::new();
    let mut bad = Vec::new();
    for arg in args {
        let found: Vec<String> = detect::split_urls(arg).into_iter().filter(|u| launch::is_web_link(u)).collect();
        if found.is_empty() {
            bad.push(arg.clone());
        }
        urls.extend(found);
    }
    if bad.is_empty() { Ok(urls) } else { Err(bad) }
}

pub fn run(args: Args) -> ExitCode {
    let urls = match links(&args.urls) {
        Ok(urls) => urls,
        Err(bad) => {
            for arg in bad {
                eprintln!("Kein gültiger Link: {arg}");
            }
            eprintln!("Links beginnen mit https://, etwa https://www.youtube.com/watch?v=…");
            return ExitCode::from(USAGE);
        }
    };
    let cfg = config(&args, Config::load());
    let start_at = args.at.map(|(h, m)| schedule::to_unix(schedule::next_at(schedule::local_now(), h, m)));
    let mode = match (args.json, args.quiet) {
        (true, _) => Mode::Json,
        (_, true) => Mode::Quiet,
        _ if term::interactive() => Mode::Live,
        _ => Mode::Plain,
    };
    sys::kill_children_on_exit();
    ExitCode::from(runtime().block_on(download(urls, cfg, start_at, Report::new(mode))))
}

/// Starts the engine for this run only: no journal, no browser bridge, no app updates.
pub fn start_engine(cfg: Config) -> (UnboundedSender<Command>, UnboundedReceiver<Event>) {
    let (tx, rx) = unbounded_channel();
    let sink: engine::Sink = Arc::new(move |ev| {
        let _ = tx.send(ev);
    });
    let cmd = engine::start(engine::Start {
        sink,
        base: config::base_dir(),
        cfg,
        journal: None,
        listener: None,
        launch: Default::default(),
        check_updates: false,
    });
    (cmd, rx)
}

/// Runtime for the terminal side; the engine brings its own.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("Laufzeitumgebung")
}

/// One message per Ctrl+C (or SIGTERM, how services are stopped).
pub fn interrupts() -> UnboundedReceiver<()> {
    let (tx, rx) = unbounded_channel();
    tokio::spawn(async move {
        while signal().await {
            if tx.send(()).is_err() {
                break;
            }
        }
    });
    rx
}

/// Waits for the next signal; `false` if none can be received.
async fn signal() -> bool {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            return tokio::select! {
                r = tokio::signal::ctrl_c() => r.is_ok(),
                r = term.recv() => r.is_some(),
            };
        }
    }
    tokio::signal::ctrl_c().await.is_ok()
}

/// Runs the downloads to the end and returns the exit code.
async fn download(urls: Vec<String>, cfg: Config, start_at: Option<i64>, mut report: Report) -> u8 {
    let (cmd, mut events) = start_engine(cfg.clone());
    let mut s = Session { expected: urls.len(), ..Default::default() };
    let _ = cmd.send(Command::Add { urls, cfg, start_at });

    let mut stop = interrupts();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Set once everything was cancelled: the jobs get until then to report back.
    let mut deadline: Option<tokio::time::Instant> = None;
    let give_up = |deadline: &mut Option<tokio::time::Instant>| {
        let _ = cmd.send(Command::CancelAll);
        deadline.get_or_insert_with(|| tokio::time::Instant::now() + GRACE);
    };

    while !s.finished() {
        tokio::select! {
            ev = events.recv() => match ev {
                Some(Event::Deps(d)) => {
                    if s.deps(d, &mut report) {
                        give_up(&mut deadline);
                    }
                }
                Some(Event::Job(id, update)) => s.job(id, update, &mut report),
                Some(_) => {}
                None => break,
            },
            _ = tick.tick() => report.tick(&s),
            Some(()) = stop.recv() => {
                // A second Ctrl+C does not wait any longer.
                if s.interrupted {
                    break;
                }
                s.interrupted = true;
                give_up(&mut deadline);
            }
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => break,
        }
    }
    let code = exit_code(&counts(&s.jobs), s.interrupted, s.tools_error.is_some());
    report.finish(&s, code);
    code
}

#[derive(Default)]
struct Session {
    jobs: Jobs,
    /// Top-level jobs to wait for, one per link.
    expected: usize,
    /// Tool setup at work, e.g. "Lade ffmpeg … 45 %".
    busy: Option<String>,
    tools_checked: bool,
    /// The tools are unusable, so nothing can start.
    tools_error: Option<String>,
    interrupted: bool,
}

impl Session {
    /// `true` if the tools cannot be used and the downloads must be given up.
    fn deps(&mut self, d: DepsStatus, report: &mut Report) -> bool {
        report.deps(&d);
        if d.busy.is_some() {
            self.busy = d.busy;
            return false;
        }
        self.busy = None;
        let first = !std::mem::replace(&mut self.tools_checked, true);
        if !d.ready() {
            let err = d.error.unwrap_or_else(|| "yt-dlp oder ffmpeg fehlen".into());
            report.fatal(&format!("Werkzeuge konnten nicht eingerichtet werden: {err}"));
            self.tools_error = Some(err);
            return true;
        }
        if first {
            report.hints(&hints(&d));
        }
        false
    }

    fn job(&mut self, id: JobId, update: JobUpdate, report: &mut Report) {
        let progress = matches!(update, JobUpdate::Progress { .. });
        let before = self.jobs.get(id).map(|j| j.state.clone());
        self.jobs.apply(id, update);
        report.job(&self.jobs, id, before.as_ref(), progress);
    }

    fn finished(&self) -> bool {
        self.jobs.top_level().count() >= self.expected && self.jobs.all_finished()
    }
}

/// Worth knowing before the downloads start, though they can go ahead.
fn hints(d: &DepsStatus) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(e) = &d.error {
        lines.push(format!("Nicht alle Werkzeuge sind bereit: {e}"));
    }
    if d.js.is_none() {
        lines.push("Keine JavaScript-Laufzeit gefunden, YouTube braucht eine.".into());
    }
    if let Some(age) = d.ytdlp_age_days.filter(|&a| a > STALE_DAYS) {
        lines.push(format!(
            "yt-dlp wurde seit {age} Tagen nicht aktualisiert. Scheitern Downloads, hilft „omnidl-cli tools update“."
        ));
    }
    lines
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    done: usize,
    failed: usize,
    no_match: usize,
    cancelled: usize,
    /// Still running when the program gave up waiting.
    open: usize,
}

/// Outcome per file: single jobs and the entries of lists, not the lists themselves.
fn counts(jobs: &Jobs) -> Counts {
    let mut c = Counts::default();
    for job in jobs.list.iter().filter(|j| j.children.is_empty()) {
        match job.state {
            JobState::Done => c.done += 1,
            JobState::Failed(_) => c.failed += 1,
            JobState::NoMatch => c.no_match += 1,
            JobState::Cancelled => c.cancelled += 1,
            _ => c.open += 1,
        }
    }
    c
}

/// "3 fertig, 1 fehlgeschlagen".
fn summary(c: &Counts) -> String {
    let parts: Vec<String> = [
        (c.done, "fertig"),
        (c.failed, "fehlgeschlagen"),
        (c.no_match, "ohne Treffer"),
        (c.cancelled, "gestoppt"),
        (c.open, "nicht fertig"),
    ]
    .into_iter()
    .filter(|&(n, _)| n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect();
    if parts.is_empty() { "Nichts heruntergeladen".into() } else { parts.join(", ") }
}

/// 0: everything arrived; 1: something did not; 130: stopped with Ctrl+C.
fn exit_code(c: &Counts, interrupted: bool, tools_failed: bool) -> u8 {
    if interrupted {
        INTERRUPTED
    } else if tools_failed || c.done == 0 || c.failed + c.no_match + c.cancelled + c.open > 0 {
        1
    } else {
        0
    }
}

/// Every file that arrived.
fn files(jobs: &Jobs) -> Vec<PathBuf> {
    jobs.list.iter().filter(|j| j.children.is_empty()).filter_map(|j| j.output.clone()).collect()
}

/// Where to look afterwards: each file, and a list's folder instead of its entries.
fn outputs(jobs: &Jobs) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for job in jobs.top_level() {
        let path = match jobs.children_of(job.id).find_map(|c| c.output.clone()) {
            Some(file) => file.parent().map(Path::to_path_buf),
            None => job.output.clone(),
        };
        if let Some(p) = path.filter(|p| !out.contains(p)) {
            out.push(p);
        }
    }
    out
}

/// The live lines: tool setup, then every unfinished job with the children
/// at work or gone wrong. Never taller than the terminal.
fn region(s: &Session, frame: usize, (width, height): (usize, usize), color: bool) -> Vec<String> {
    let mut rows = Vec::new();
    if let Some(busy) = &s.busy {
        let sym = (term::spinner(frame), Paint::Blue);
        rows.push(term::row(0, sym, "Werkzeuge", (busy, Paint::Dim), width, color));
    }
    for job in s.jobs.top_level().filter(|j| !j.state.is_finished()) {
        rows.push(term::job_row(job, false, frame, width, color));
        let shown = |c: &&Job| {
            matches!(
                c.state,
                JobState::Resolving | JobState::Downloading | JobState::Processing | JobState::Failed(_) | JobState::NoMatch
            )
        };
        for child in s.jobs.children_of(job.id).filter(shown) {
            rows.push(term::job_row(child, true, frame, width, color));
        }
    }
    let max = height.saturating_sub(1).max(3);
    if rows.len() > max {
        let more = rows.len() - (max - 1);
        rows.truncate(max - 1);
        rows.push(term::paint(&format!("  … und {more} weitere"), Paint::Dim, color));
    }
    rows
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Redrawn in place, for a terminal.
    Live,
    /// A line per step, for pipes and logs.
    Plain,
    Json,
    /// Only the finished files, errors on stderr.
    Quiet,
}

/// What was said about a job, so plain and JSON output do not repeat themselves.
#[derive(Default)]
struct Said {
    scheduled: bool,
    started: bool,
    processing: bool,
    last: Option<Instant>,
}

struct Report {
    mode: Mode,
    live: Option<Live>,
    frame: usize,
    /// Lines the live display prints for good with its next redraw.
    log: Vec<String>,
    said: HashMap<JobId, Said>,
    steps: term::Steps,
}

impl Report {
    fn new(mode: Mode) -> Self {
        let live = (mode == Mode::Live).then(Live::new);
        Self { mode, live, frame: 0, log: Vec::new(), said: HashMap::new(), steps: term::Steps::default() }
    }

    fn color(&self) -> bool {
        self.live.as_ref().is_some_and(|l| l.color)
    }

    fn deps(&mut self, d: &DepsStatus) {
        match (self.mode, &d.busy) {
            (Mode::Json, _) => println!("{}", json!({ "event": "tools", "tools": d })),
            // The routine check at every start is not worth a line.
            (Mode::Plain, Some(busy)) if !busy.starts_with("Prüfe") && self.steps.due(busy, Instant::now()) => {
                println!("Werkzeuge: {busy}");
            }
            _ => {}
        }
    }

    fn hints(&mut self, lines: &[String]) {
        match self.mode {
            Mode::Live => {
                let mark = term::paint("!", Paint::Yellow, self.color());
                self.log.extend(lines.iter().map(|l| format!("{mark} {l}")));
            }
            Mode::Plain => lines.iter().for_each(|l| println!("Hinweis: {l}")),
            Mode::Json | Mode::Quiet => {}
        }
    }

    fn fatal(&mut self, msg: &str) {
        match self.mode {
            Mode::Live => self.log.push(format!("{} {msg}", term::paint("✗", Paint::Red, self.color()))),
            Mode::Plain | Mode::Quiet => eprintln!("{msg}"),
            // The summary carries it.
            Mode::Json => {}
        }
    }

    fn job(&mut self, jobs: &Jobs, id: JobId, before: Option<&JobState>, progress: bool) {
        let Some(job) = jobs.get(id) else { return };
        let child = job.parent.is_some();
        let finished_now = job.state.is_finished() && !before.is_some_and(JobState::is_finished);
        let now = Instant::now();
        match self.mode {
            // Finished jobs leave the live lines and stay above them, with what went wrong in a list.
            Mode::Live if finished_now && !child => {
                let (width, _) = term::size();
                let color = self.color();
                self.log.push(term::job_row(job, false, 0, width, color));
                let failed = jobs.children_of(id).filter(|c| c.state.can_retry());
                self.log.extend(failed.map(|c| term::job_row(c, true, 0, width, color)));
            }
            Mode::Plain => {
                let said = self.said.entry(id).or_default();
                if let Some(line) = plain_line(said, job, finished_now, progress, now) {
                    println!("{line}");
                }
            }
            Mode::Json => {
                let said = self.said.entry(id).or_default();
                let throttled = progress && said.last.is_some_and(|t| now.duration_since(t) < JSON_PROGRESS);
                if progress && !throttled {
                    said.last = Some(now);
                }
                if !throttled {
                    let (status, tone) = job.status(child);
                    println!("{}", json!({ "event": "job", "job": job, "status": status, "tone": tone }));
                }
            }
            Mode::Live | Mode::Quiet => {}
        }
    }

    fn tick(&mut self, s: &Session) {
        let Some(live) = &mut self.live else { return };
        self.frame += 1;
        let region = region(s, self.frame, term::size(), live.color);
        live.draw(&std::mem::take(&mut self.log), &region);
    }

    fn finish(&mut self, s: &Session, code: u8) {
        let counts = counts(&s.jobs);
        let open: Vec<&Job> = s.jobs.top_level().filter(|j| !j.state.is_finished()).collect();
        match self.mode {
            Mode::Live | Mode::Plain => {
                // Whatever is still open after giving up is shown as it stands.
                if let Some(mut live) = self.live.take() {
                    let (width, _) = term::size();
                    let mut log = std::mem::take(&mut self.log);
                    log.extend(open.iter().map(|j| term::job_row(j, false, 0, width, live.color)));
                    live.close(&log);
                } else {
                    open.iter().for_each(|j| println!("{}", term::job_line(j, false)));
                }
                let head = summary(&counts);
                println!();
                println!("{}", if s.interrupted { format!("Abgebrochen · {head}") } else { head });
                outputs(&s.jobs).iter().for_each(|p| println!("  {}", p.display()));
            }
            Mode::Json => println!(
                "{}",
                json!({
                    "event": "summary",
                    "done": counts.done,
                    "failed": counts.failed,
                    "no_match": counts.no_match,
                    "cancelled": counts.cancelled,
                    "open": counts.open,
                    "files": files(&s.jobs),
                    "interrupted": s.interrupted,
                    "error": s.tools_error,
                    "exit": code,
                })
            ),
            Mode::Quiet => {
                let leaves = s.jobs.list.iter().filter(|j| j.children.is_empty());
                leaves.filter(|j| !matches!(j.state, JobState::Done)).for_each(|j| eprintln!("{}", term::job_line(j, false)));
                files(&s.jobs).iter().for_each(|f| println!("{}", f.display()));
            }
        }
    }
}

/// Plain output speaks when a job is scheduled, starts, converts or ends, and
/// repeats its progress every few seconds.
fn plain_line(said: &mut Said, job: &Job, finished_now: bool, progress: bool, now: Instant) -> Option<String> {
    let due = said.last.is_none_or(|t| now.duration_since(t) >= PLAIN_PROGRESS);
    let say = match job.state {
        _ if finished_now => true,
        JobState::Scheduled(_) => !std::mem::replace(&mut said.scheduled, true),
        JobState::Resolving | JobState::Downloading | JobState::Group => {
            !std::mem::replace(&mut said.started, true) || (progress && due)
        }
        JobState::Processing => !std::mem::replace(&mut said.processing, true),
        _ => false,
    };
    say.then(|| {
        said.last = Some(now);
        term::job_line(job, job.parent.is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: Args,
    }

    fn parse(line: &[&str]) -> Result<Args, clap::Error> {
        Cli::try_parse_from(std::iter::once("get").chain(line.iter().copied())).map(|c| c.args)
    }

    const URL: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

    fn settings() -> Config {
        let mut cfg = Config { download_dir: PathBuf::from("D:/Videos"), parallel: 3, ..Config::default() };
        cfg.set_format(Format::Flac);
        cfg.set_format(Format::Mkv);
        cfg
    }

    #[test]
    fn without_flags_the_settings_apply() {
        assert_eq!(config(&parse(&[URL]).unwrap(), settings()), settings());
    }

    #[test]
    fn flags_override_the_settings_for_this_run() {
        let music = std::env::temp_dir().join("Musik");
        let args = parse(&[
            URL,
            "-f",
            "mp3",
            "--bitrate",
            "192",
            "-q",
            "720",
            "-o",
            music.to_str().unwrap(),
            "-j",
            "8",
            "--no-playlist",
            "--cookies",
            "firefox",
        ])
        .unwrap();
        let cfg = config(&args, settings());
        assert_eq!(cfg.format, Format::Mp3);
        assert_eq!(cfg.audio_quality, AudioQuality::K192);
        assert_eq!(cfg.video_quality, VideoQuality::P720);
        assert_eq!(cfg.download_dir, music);
        assert_eq!(cfg.parallel, 8);
        assert!(!cfg.playlist);
        assert_eq!(cfg.cookies, Cookies::Firefox);
        assert_eq!(config(&parse(&[URL, "--cookies", "off"]).unwrap(), cfg).cookies, Cookies::Off);
    }

    #[test]
    fn audio_uses_the_remembered_audio_format() {
        assert_eq!(config(&parse(&[URL, "-a"]).unwrap(), settings()).format, Format::Flac);
        assert_eq!(config(&parse(&["-a", URL]).unwrap(), Config::default()).format, Format::Mp3);
    }

    #[test]
    fn relative_folders_become_absolute() {
        let cfg = config(&parse(&[URL, "-o", "downloads"]).unwrap(), settings());
        assert!(cfg.download_dir.is_absolute());
        assert!(cfg.download_dir.ends_with("downloads"));
    }

    #[test]
    fn contradictions_and_nonsense_are_usage_errors() {
        for line in [
            &[URL, "-a", "-f", "mp4"][..],
            &[URL, "-j", "17"],
            &[URL, "-j", "0"],
            &[URL, "-q", "999"],
            &[URL, "-f", "avi"],
            &[URL, "--at", "25:00"],
            &[URL, "--json", "--quiet"],
            &["--quiet"],
        ] {
            let err = parse(line).err().unwrap_or_else(|| panic!("{line:?} müsste scheitern"));
            assert_eq!(err.exit_code(), i32::from(USAGE), "{line:?}");
        }
    }

    #[test]
    fn start_times_are_clock_times() {
        assert_eq!(parse_at("2:30"), Ok((2, 30)));
        assert_eq!(parse_at("02:30"), Ok((2, 30)));
        assert_eq!(parse_at(" 23:59 "), Ok((23, 59)));
        assert_eq!(parse_at("0:00"), Ok((0, 0)));
        for bad in ["24:00", "12:60", "2:3", "2", "2.30", "+1:30", "123:00", "ab:cd", ""] {
            assert!(parse_at(bad).is_err(), "{bad}");
        }
        assert!(parse_at("25:00").unwrap_err().contains("HH:MM"));
        assert_eq!(parse(&[URL, "--at", "7:05"]).unwrap().at, Some((7, 5)));
    }

    #[test]
    fn arguments_must_be_web_links() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            links(&args(&[URL, "youtu.be/abc", "https://a.com/1 https://b.com/2"])),
            Ok(args(&[URL, "https://youtu.be/abc", "https://a.com/1", "https://b.com/2"]))
        );
        assert_eq!(links(&args(&[URL, "Katzenvideo", "file:///C:/x"])), Err(args(&["Katzenvideo", "file:///C:/x"])));
    }

    fn counted(done: usize, failed: usize, no_match: usize, cancelled: usize, open: usize) -> Counts {
        Counts { done, failed, no_match, cancelled, open }
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(&counted(3, 0, 0, 0, 0), false, false), 0);
        assert_eq!(exit_code(&counted(3, 1, 0, 0, 0), false, false), 1, "etwas fehlgeschlagen");
        assert_eq!(exit_code(&counted(3, 0, 1, 0, 0), false, false), 1, "kein Treffer");
        assert_eq!(exit_code(&counted(3, 0, 0, 1, 0), false, false), 1, "gestoppt");
        assert_eq!(exit_code(&counted(0, 0, 0, 0, 0), false, false), 1, "nichts geladen");
        assert_eq!(exit_code(&counted(0, 0, 0, 1, 0), false, true), 1, "Werkzeuge fehlen");
        assert_eq!(exit_code(&counted(1, 0, 0, 2, 0), true, false), INTERRUPTED);
    }

    #[test]
    fn summaries_read_naturally() {
        assert_eq!(summary(&counted(3, 1, 0, 0, 0)), "3 fertig, 1 fehlgeschlagen");
        assert_eq!(summary(&counted(1, 0, 0, 0, 0)), "1 fertig");
        assert_eq!(summary(&counted(0, 0, 2, 1, 1)), "2 ohne Treffer, 1 gestoppt, 1 nicht fertig");
        assert_eq!(summary(&Counts::default()), "Nichts heruntergeladen");
    }

    fn add(jobs: &mut Jobs, id: JobId, parent: Option<JobId>, title: &str) {
        let url = format!("https://example.com/{id}");
        jobs.apply(id, JobUpdate::New { parent, url, title: title.into(), source: "YouTube" });
    }

    /// A playlist with one file done and one failed, plus a single video.
    fn finished_list() -> Jobs {
        let mut jobs = Jobs::default();
        add(&mut jobs, 1, None, "Playlist");
        add(&mut jobs, 2, Some(1), "Eins");
        add(&mut jobs, 3, Some(1), "Zwei");
        add(&mut jobs, 4, None, "Video");
        jobs.apply(2, JobUpdate::Output(PathBuf::from("D:/dl/Playlist/001 - Eins.mp4")));
        jobs.apply(2, JobUpdate::State(JobState::Done));
        jobs.apply(3, JobUpdate::State(JobState::Failed("Video unavailable".into())));
        jobs.apply(1, JobUpdate::State(JobState::Failed("1 von 2 fehlgeschlagen".into())));
        jobs.apply(4, JobUpdate::Output(PathBuf::from("D:/dl/Video.mp4")));
        jobs.apply(4, JobUpdate::State(JobState::Done));
        jobs
    }

    #[test]
    fn counts_files_not_lists() {
        let jobs = finished_list();
        assert_eq!(counts(&jobs), counted(2, 1, 0, 0, 0));
        assert_eq!(files(&jobs), vec![PathBuf::from("D:/dl/Playlist/001 - Eins.mp4"), PathBuf::from("D:/dl/Video.mp4")]);
        assert_eq!(outputs(&jobs), vec![PathBuf::from("D:/dl/Playlist"), PathBuf::from("D:/dl/Video.mp4")]);
    }

    #[test]
    fn waits_for_every_link() {
        let mut s = Session { expected: 2, ..Default::default() };
        assert!(!s.finished(), "noch nichts gemeldet");
        add(&mut s.jobs, 1, None, "Eins");
        s.jobs.apply(1, JobUpdate::State(JobState::Done));
        assert!(!s.finished(), "der zweite Link fehlt noch");
        add(&mut s.jobs, 2, None, "Zwei");
        s.jobs.apply(2, JobUpdate::State(JobState::Cancelled));
        assert!(s.finished());
    }

    #[test]
    fn live_lines_show_what_is_at_work() {
        let mut s = Session { busy: Some("Lade ffmpeg … 45 %".into()), ..Default::default() };
        add(&mut s.jobs, 1, None, "Fertig");
        add(&mut s.jobs, 2, None, "Playlist");
        for (id, state) in [(3, JobState::Done), (4, JobState::Downloading), (5, JobState::Queued), (6, JobState::NoMatch)] {
            add(&mut s.jobs, id, Some(2), &format!("Kind {id}"));
            s.jobs.apply(id, JobUpdate::State(state));
        }
        s.jobs.apply(1, JobUpdate::State(JobState::Done));
        s.jobs.apply(2, JobUpdate::State(JobState::Group));
        s.jobs.apply(2, JobUpdate::Item { index: 2, count: 4 });
        let rows = region(&s, 0, (80, 24), false);
        assert_eq!(
            rows,
            vec![
                "⠋ Werkzeuge  Lade ffmpeg … 45 %",
                "⠋ Playlist  YouTube · 2 von 4 fertig",
                "  ⠋ Kind 4  Lädt …",
                "  ! Kind 6  Kein passender Treffer",
            ]
        );
        let rows = region(&s, 0, (80, 4), false);
        assert_eq!(rows.len(), 3, "nie höher als das Terminal");
        assert_eq!(rows[2], "  … und 2 weitere");
    }

    #[test]
    fn plain_output_names_the_steps() {
        let mut jobs = Jobs::default();
        add(&mut jobs, 1, None, "Me at the zoo");
        let mut said = Said::default();
        let t0 = Instant::now();
        let mut step = |jobs: &mut Jobs, update: JobUpdate, secs: u64| {
            let progress = matches!(update, JobUpdate::Progress { .. });
            let before = jobs.get(1).unwrap().state.clone();
            jobs.apply(1, update);
            let job = jobs.get(1).unwrap();
            let finished_now = job.state.is_finished() && !before.is_finished();
            plain_line(&mut said, job, finished_now, progress, t0 + Duration::from_secs(secs))
        };
        assert_eq!(step(&mut jobs, JobUpdate::State(JobState::Queued), 0), None);
        assert_eq!(step(&mut jobs, JobUpdate::State(JobState::Downloading), 0).unwrap(), "Me at the zoo · YouTube · Lädt …");
        let progress = |frac| JobUpdate::Progress { frac: Some(frac), speed: None, eta: None };
        assert_eq!(step(&mut jobs, progress(0.1), 1), None, "gerade erst gemeldet");
        assert_eq!(step(&mut jobs, JobUpdate::State(JobState::Downloading), 2), None, "zweiter Datenstrom");
        assert_eq!(step(&mut jobs, progress(0.6), 12).unwrap(), "Me at the zoo · YouTube · 60 %");
        assert_eq!(step(&mut jobs, JobUpdate::State(JobState::Processing), 13).unwrap(), "Me at the zoo · YouTube · Wird umgewandelt …");
        assert_eq!(step(&mut jobs, JobUpdate::State(JobState::Processing), 13), None);
        assert_eq!(step(&mut jobs, JobUpdate::State(JobState::Done), 14).unwrap(), "Me at the zoo · YouTube · Fertig");
    }

    #[test]
    fn hints_before_starting() {
        let d = DepsStatus { ytdlp: Some("2026.01.01".into()), ytdlp_age_days: Some(30), ffmpeg: true, ..Default::default() };
        let lines = hints(&d);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("JavaScript"));
        assert!(lines[1].contains("seit 30 Tagen"));
        let fresh = DepsStatus { ytdlp_age_days: Some(2), js: Some("deno".into()), ..d };
        assert!(hints(&fresh).is_empty());
    }
}
