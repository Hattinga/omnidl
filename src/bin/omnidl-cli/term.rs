//! Terminal output: the live display redrawn in place, and the line
//! formatting it shares with plain output for pipes and logs.

use omnidl::job::JobState;
use omnidl::jobs::{Job, Tone};
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Whether stdout is a terminal that can take the live display.
pub fn interactive() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("TERM").is_none_or(|t| t != "dumb") && ansi()
}

#[cfg(windows)]
fn ansi() -> bool {
    crossterm::ansi_support::supports_ansi()
}

#[cfg(not(windows))]
fn ansi() -> bool {
    true
}

/// Colors unless `NO_COLOR` asks for none (no-color.org).
pub fn colors() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// Columns and rows; a classic 80×24 if unknown.
pub fn size() -> (usize, usize) {
    match crossterm::terminal::size() {
        Ok((w, h)) if w > 0 && h > 0 => (w as usize, h as usize),
        _ => (80, 24),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    Plain,
    Dim,
    Blue,
    Green,
    Yellow,
    Red,
}

impl Paint {
    fn code(self) -> &'static str {
        match self {
            Paint::Plain => "",
            Paint::Dim => "\x1b[2m",
            Paint::Blue => "\x1b[34m",
            Paint::Green => "\x1b[32m",
            Paint::Yellow => "\x1b[33m",
            Paint::Red => "\x1b[31m",
        }
    }

    pub fn of(tone: Tone) -> Paint {
        match tone {
            Tone::Normal => Paint::Dim,
            Tone::Warning => Paint::Yellow,
            Tone::Error => Paint::Red,
        }
    }
}

pub fn paint(s: &str, p: Paint, color: bool) -> String {
    if !color || p == Paint::Plain || s.is_empty() { s.to_string() } else { format!("{}{s}\x1b[0m", p.code()) }
}

fn width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Control characters (tabs, line breaks in titles) become spaces.
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Cuts `s` to at most `max` columns, ending in "…" when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    let clean = clean(s);
    if width(&clean) <= max {
        return clean;
    }
    let mut out = String::new();
    let mut used = 0;
    for c in clean.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        used += w;
        out.push(c);
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// Title and status for one line of `room` columns, two spaces apart. The
/// status stays whole as long as the title keeps a readable start.
pub fn fit(title: &str, status: &str, room: usize) -> (String, String) {
    const MIN_TITLE: usize = 16;
    let (tw, sw) = (width(title), width(status));
    if sw == 0 {
        return (truncate(title, room), String::new());
    }
    if tw + 2 + sw <= room {
        return (title.to_string(), status.to_string());
    }
    let keep = tw.min(MIN_TITLE);
    if keep + 2 + sw <= room {
        return (truncate(title, room - 2 - sw), status.to_string());
    }
    if room < keep + 3 {
        return (truncate(title, room), String::new());
    }
    (truncate(title, keep), truncate(status, room - keep - 2))
}

/// Symbol in front of a job: a spinner while it works.
pub fn symbol(state: &JobState, frame: usize) -> (&'static str, Paint) {
    match state {
        JobState::Done => ("✓", Paint::Green),
        JobState::Failed(_) => ("✗", Paint::Red),
        JobState::NoMatch => ("!", Paint::Yellow),
        JobState::Cancelled => ("–", Paint::Dim),
        JobState::Queued | JobState::Scheduled(_) => ("·", Paint::Dim),
        JobState::Resolving | JobState::Downloading | JobState::Processing | JobState::Group => {
            (SPINNER[frame % SPINNER.len()], Paint::Blue)
        }
    }
}

pub fn spinner(frame: usize) -> &'static str {
    SPINNER[frame % SPINNER.len()]
}

/// "⠹ Title  status" in at most `width` columns (one spare against wrapping).
pub fn row(indent: usize, sym: (&str, Paint), title: &str, status: (&str, Paint), width: usize, color: bool) -> String {
    let room = width.saturating_sub(indent + 3);
    let (title, text) = fit(title, status.0, room);
    let mut line = format!("{}{} {title}", " ".repeat(indent), paint(sym.0, sym.1, color));
    if !text.is_empty() {
        let _ = write!(line, "  {}", paint(&text, status.1, color));
    }
    line
}

/// One job for the live display; children sit under their group's title.
pub fn job_row(job: &Job, child: bool, frame: usize, width: usize, color: bool) -> String {
    let (status, tone) = job.status(child);
    let indent = if child { 2 } else { 0 };
    row(indent, symbol(&job.state, frame), &job.title, (&status, Paint::of(tone)), width, color)
}

/// One job as a plain line: "Title · YouTube · 45 %", failures marked for grep.
pub fn job_line(job: &Job, child: bool) -> String {
    let (status, tone) = job.status(child);
    let indent = if child { "  " } else { "" };
    let mark = if tone == Tone::Error { "Fehler: " } else { "" };
    format!("{indent}{mark}{} · {status}", clean(&job.title))
}

/// Tool setup messages as plain lines: each step once, its progress every few seconds.
#[derive(Default)]
pub struct Steps {
    last: Option<(String, Instant)>,
}

impl Steps {
    pub fn due(&mut self, msg: &str, now: Instant) -> bool {
        // "Lade ffmpeg … 45 %" is the step "Lade ffmpeg ".
        let step = msg.split('…').next().unwrap_or(msg);
        let due = match &self.last {
            Some((last, at)) => last != step || now.duration_since(*at) >= Duration::from_secs(5),
            None => true,
        };
        if due {
            self.last = Some((step.to_string(), now));
        }
        due
    }
}

/// Region redrawn in place below everything printed for good.
pub struct Live {
    drawn: usize,
    pub color: bool,
}

impl Live {
    pub fn new() -> Self {
        print(HIDE_CURSOR);
        Self { drawn: 0, color: colors() }
    }

    /// Prints `log` for good, then replaces the live lines with `region`.
    pub fn draw(&mut self, log: &[String], region: &[String]) {
        print(&frame(self.drawn, log, region));
        self.drawn = region.len();
    }

    /// Clears the live lines and gives the cursor back.
    pub fn close(&mut self, log: &[String]) {
        self.draw(log, &[]);
        print(SHOW_CURSOR);
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        print(SHOW_CURSOR);
    }
}

const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

/// Moves up over the `drawn` lines, writes `log` and `region` line by line
/// (clearing each rest) and wipes whatever the old region left below.
fn frame(drawn: usize, log: &[String], region: &[String]) -> String {
    let mut s = String::new();
    if drawn > 0 {
        // `ESC[0A` would still move one line in some terminals.
        let _ = write!(s, "\x1b[{drawn}A");
    }
    s.push('\r');
    for line in log.iter().chain(region) {
        s.push_str(line);
        s.push_str("\x1b[K\n");
    }
    s.push_str("\x1b[J");
    s
}

fn print(s: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnidl::job::JobUpdate;
    use omnidl::jobs::Jobs;

    #[test]
    fn truncation_counts_columns() {
        assert_eq!(truncate("Me at the zoo", 20), "Me at the zoo");
        assert_eq!(truncate("Me at the zoo", 8), "Me at t…");
        assert_eq!(width(&truncate("日本語のタイトル", 7)), 7, "breite Zeichen zählen doppelt");
        assert_eq!(truncate("日本語のタイトル", 7), "日本語…");
        assert_eq!(truncate("a\tb\nc", 10), "a b c", "Steuerzeichen werden Leerzeichen");
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn status_wins_over_a_long_title() {
        let title = "A really long video title that goes on and on";
        let status = "YouTube · 45 % · 3.8 MB/s · noch 0:12";
        assert_eq!(fit(title, status, 200), (title.to_string(), status.to_string()));
        let (t, s) = fit(title, status, 60);
        assert_eq!(s, status);
        assert_eq!(width(&t) + 2 + width(&s), 60);
        // Too narrow even for that: the title keeps a readable start, the status is cut.
        let (t, s) = fit(title, status, 40);
        assert_eq!(width(&t), 16);
        assert!(s.ends_with('…') && width(&s) == 22, "{s}");
        assert_eq!(fit(title, status, 10).1, "", "winzig: nur der Titel");
    }

    fn job(state: JobState) -> Jobs {
        let mut jobs = Jobs::default();
        let url = "https://youtu.be/jNQXAC9IVRw".to_string();
        jobs.apply(1, JobUpdate::New { parent: None, url, title: "Me at the zoo".into(), source: "YouTube" });
        jobs.apply(1, JobUpdate::State(state));
        jobs
    }

    #[test]
    fn rows_read_calmly() {
        let jobs = job(JobState::Downloading);
        let mut j = jobs.get(1).unwrap().clone();
        j.frac = Some(0.45);
        assert_eq!(job_row(&j, false, 0, 80, false), "⠋ Me at the zoo  YouTube · 45 %");
        assert_eq!(job_row(&j, false, 2, 80, true), "\x1b[34m⠹\x1b[0m Me at the zoo  \x1b[2mYouTube · 45 %\x1b[0m");
        assert_eq!(job_row(&j, true, 0, 80, false), "  ⠋ Me at the zoo  45 %", "Kinder eingerückt, ohne Quelle");

        let done = job(JobState::Done);
        assert_eq!(job_row(done.get(1).unwrap(), false, 0, 80, false), "✓ Me at the zoo  YouTube · Fertig");
        assert_eq!(job_line(done.get(1).unwrap(), false), "Me at the zoo · YouTube · Fertig");

        let failed = job(JobState::Failed("[youtube] x: Video unavailable".into()));
        let row = job_row(failed.get(1).unwrap(), false, 0, 80, true);
        assert!(row.starts_with("\x1b[31m✗\x1b[0m") && row.ends_with("\x1b[31mYouTube · Video unavailable\x1b[0m"), "{row:?}");
        assert_eq!(job_line(failed.get(1).unwrap(), false), "Fehler: Me at the zoo · YouTube · Video unavailable");
    }

    #[test]
    fn rows_never_wrap() {
        let mut jobs = job(JobState::Downloading);
        jobs.apply(1, JobUpdate::Title("Ein sehr langer Titel, der in kein schmales Fenster passt".into()));
        jobs.apply(1, JobUpdate::Progress { frac: Some(0.5), speed: Some(1e6), eta: Some(30) });
        for w in [20, 40, 60, 80] {
            let line = job_row(jobs.get(1).unwrap(), true, 0, w, false);
            assert!(width(&line) < w, "{w}: {line}");
        }
    }

    #[test]
    fn tool_steps_are_told_once_with_occasional_progress() {
        let mut steps = Steps::default();
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        assert!(steps.due("Lade yt-dlp …", at(0)));
        assert!(!steps.due("Lade yt-dlp … 10 %", at(1)));
        assert!(steps.due("Lade yt-dlp … 80 %", at(6)), "nach ein paar Sekunden wieder");
        assert!(steps.due("Lade ffmpeg …", at(7)), "neuer Schritt sofort");
    }

    #[test]
    fn frames_redraw_in_place() {
        let region = ["a".to_string(), "b".to_string()];
        assert_eq!(frame(0, &[], &region), "\ra\x1b[K\nb\x1b[K\n\x1b[J");
        let log = ["fertig".to_string()];
        assert_eq!(frame(2, &log, &region[..1]), "\x1b[2A\rfertig\x1b[K\na\x1b[K\n\x1b[J");
        assert_eq!(frame(1, &[], &[]), "\x1b[1A\r\x1b[J", "leer: nur wegwischen");
    }
}
