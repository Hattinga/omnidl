//! `omnidl-cli tools`: show or update yt-dlp, ffmpeg, the JavaScript runtime and gallery-dl.

use crate::get::{self, INTERRUPTED};
use crate::term::{self, Live, Paint};
use omnidl::config::{self, Config};
use omnidl::deps::DepsStatus;
use omnidl::engine::Command;
use omnidl::job::Event;
use omnidl::sys;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

/// yt-dlp older than this often fails on YouTube.
pub const STALE_DAYS: i64 = 14;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Lädt die neueste yt-dlp-Version (hilft, wenn Downloads plötzlich scheitern).
    Update,
}

pub fn run(args: Args) -> ExitCode {
    sys::kill_children_on_exit();
    let code = get::runtime().block_on(async {
        // The engine checks the tools on start and installs what is missing.
        let (cmd, mut events) = get::start_engine(Config::default());
        let mut progress = Progress::new();
        let before = match settle(&mut events, &mut progress).await {
            Ok(d) => d,
            Err(code) => return progress.abort(code),
        };
        match args.action {
            None => {
                progress.close();
                show(&before)
            }
            Some(Action::Update) => {
                let _ = cmd.send(Command::UpdateTools);
                let after = match settle(&mut events, &mut progress).await {
                    Ok(d) => d,
                    Err(code) => return progress.abort(code),
                };
                progress.close();
                let (line, ok) = updated(&before, &after);
                if ok {
                    println!("{line}");
                    0
                } else {
                    eprintln!("{line}");
                    1
                }
            }
        }
    });
    ExitCode::from(code)
}

/// Waits until the running check or install is through, showing its progress.
async fn settle(events: &mut UnboundedReceiver<Event>, progress: &mut Progress) -> Result<DepsStatus, u8> {
    let mut stop = get::interrupts();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Some(Event::Deps(d)) if d.busy.is_none() => return Ok(d),
                Some(Event::Deps(d)) => progress.set(d.busy.unwrap_or_default()),
                Some(_) => {}
                None => return Err(1),
            },
            _ = tick.tick() => progress.tick(),
            Some(()) = stop.recv() => return Err(INTERRUPTED),
        }
    }
}

/// The table, then whatever needs attention. Exit code 1 if downloads cannot work.
fn show(d: &DepsStatus) -> u8 {
    let color = term::interactive() && term::colors();
    for (name, value, paint) in rows(d) {
        println!("{name:<21}{}", term::paint(&value, paint, color));
    }
    let folder = config::base_dir().join("bin");
    println!("{}", term::paint(&format!("\nOrdner: {}", folder.display()), Paint::Dim, color));
    if let Some(e) = &d.error {
        eprintln!("\nFehler: {e}");
        return 1;
    }
    if !d.ready() {
        eprintln!("\nyt-dlp oder ffmpeg fehlen, so geht kein Download.");
        return 1;
    }
    if d.ytdlp_age_days.is_some_and(|a| a > STALE_DAYS) {
        println!("\n„omnidl-cli tools update“ lädt die neueste yt-dlp-Version.");
    }
    0
}

/// Name, state and how to show it.
fn rows(d: &DepsStatus) -> Vec<(&'static str, String, Paint)> {
    let ytdlp = match (&d.ytdlp, d.ytdlp_age_days) {
        (Some(v), Some(age)) if age > STALE_DAYS => (format!("{v} · {} · veraltet", refreshed(age)), Paint::Yellow),
        (Some(v), Some(age)) => (format!("{v} · {}", refreshed(age)), Paint::Plain),
        (Some(v), None) => (v.clone(), Paint::Plain),
        (None, _) => ("fehlt".into(), Paint::Red),
    };
    let present = |ok: bool, missing: Paint| if ok { ("bereit".to_string(), Paint::Plain) } else { ("fehlt".to_string(), missing) };
    let js = match d.js.as_deref() {
        Some("deno") => ("Deno".to_string(), Paint::Plain),
        Some(_) => ("Node.js".to_string(), Paint::Plain),
        None => ("fehlt".to_string(), Paint::Yellow),
    };
    [("yt-dlp", ytdlp), ("ffmpeg", present(d.ffmpeg, Paint::Red)), ("JavaScript-Laufzeit", js), ("gallery-dl", present(d.gallery, Paint::Yellow))]
        .into_iter()
        .map(|(name, (value, paint))| (name, value, paint))
        .collect()
}

fn refreshed(days: i64) -> String {
    match days {
        ..=0 => "heute aktualisiert".into(),
        1 => "gestern aktualisiert".into(),
        n => format!("vor {n} Tagen aktualisiert"),
    }
}

/// What `tools update` did, and whether it worked.
fn updated(before: &DepsStatus, after: &DepsStatus) -> (String, bool) {
    if let Some(e) = &after.error {
        return (format!("yt-dlp konnte nicht aktualisiert werden: {e}"), false);
    }
    match (&before.ytdlp, &after.ytdlp) {
        (_, None) => ("yt-dlp fehlt nach dem Update.".into(), false),
        (Some(old), Some(new)) if old == new => (format!("yt-dlp ist aktuell: {new}"), true),
        (Some(old), Some(new)) => (format!("yt-dlp aktualisiert: {old} → {new}"), true),
        (None, Some(new)) => (format!("yt-dlp installiert: {new}"), true),
    }
}

/// Tool setup on the way: a spinner line in a terminal, plain lines otherwise.
struct Progress {
    live: Option<Live>,
    text: Option<String>,
    frame: usize,
    steps: term::Steps,
}

impl Progress {
    fn new() -> Self {
        let live = term::interactive().then(Live::new);
        Self { live, text: None, frame: 0, steps: term::Steps::default() }
    }

    fn set(&mut self, text: String) {
        if self.live.is_none() && self.steps.due(&text, Instant::now()) {
            println!("{text}");
        }
        self.text = Some(text);
    }

    fn tick(&mut self) {
        let (Some(live), Some(text)) = (&mut self.live, &self.text) else { return };
        self.frame += 1;
        let (width, _) = term::size();
        let line = term::row(0, (term::spinner(self.frame), Paint::Blue), text, ("", Paint::Dim), width, live.color);
        live.draw(&[], &[line]);
    }

    fn close(&mut self) {
        if let Some(mut live) = self.live.take() {
            live.close(&[]);
        }
    }

    fn abort(&mut self, code: u8) -> u8 {
        self.close();
        eprintln!("{}", if code == INTERRUPTED { "Abgebrochen." } else { "Die Werkzeug-Prüfung ist unerwartet beendet." });
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(version: &str, age: i64) -> DepsStatus {
        DepsStatus {
            ytdlp: Some(version.into()),
            ytdlp_age_days: Some(age),
            ffmpeg: true,
            js: Some("deno".into()),
            gallery: true,
            ..Default::default()
        }
    }

    #[test]
    fn table_names_every_tool() {
        let values: Vec<(&str, String)> = rows(&ready("2026.09.20", 4)).into_iter().map(|(n, v, _)| (n, v)).collect();
        assert_eq!(
            values,
            vec![
                ("yt-dlp", "2026.09.20 · vor 4 Tagen aktualisiert".into()),
                ("ffmpeg", "bereit".into()),
                ("JavaScript-Laufzeit", "Deno".into()),
                ("gallery-dl", "bereit".into()),
            ]
        );
    }

    #[test]
    fn stale_and_missing_tools_stand_out() {
        let old = rows(&ready("2026.01.01", 40));
        assert_eq!(old[0].1, "2026.01.01 · vor 40 Tagen aktualisiert · veraltet");
        assert_eq!(old[0].2, Paint::Yellow);
        let missing = rows(&DepsStatus::default());
        assert!(missing.iter().all(|(_, v, _)| v == "fehlt"));
        assert_eq!(missing[1].2, Paint::Red, "ohne ffmpeg geht nichts");
        assert_eq!(missing[3].2, Paint::Yellow, "gallery-dl nur für Fotos");
        assert_eq!(refreshed(0), "heute aktualisiert");
        assert_eq!(refreshed(1), "gestern aktualisiert");
    }

    #[test]
    fn update_reports_the_change() {
        let old = ready("2026.09.20", 4);
        assert_eq!(updated(&old, &ready("2026.09.24", 0)), ("yt-dlp aktualisiert: 2026.09.20 → 2026.09.24".into(), true));
        assert_eq!(updated(&old, &old), ("yt-dlp ist aktuell: 2026.09.20".into(), true));
        assert_eq!(updated(&DepsStatus::default(), &old).0, "yt-dlp installiert: 2026.09.20");
        let failed = DepsStatus { error: Some("yt-dlp: Download fehlgeschlagen".into()), ..old.clone() };
        assert!(!updated(&old, &failed).1);
        assert!(!updated(&old, &DepsStatus::default()).1);
    }
}
