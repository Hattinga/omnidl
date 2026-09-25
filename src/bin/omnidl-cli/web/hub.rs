//! What every browser sees: the download list, tool status and settings, kept
//! up to date from the engine's events and fanned out to open pages.
//!
//! Every change gets a sequence number under the same lock that applies it.
//! A page loads `/api/state` (which carries the current number) and then only
//! applies events that are newer, so nothing is lost between the two.

use super::files::{self, Offer};
use omnidl::config::Config;
use omnidl::deps::DepsStatus;
use omnidl::job::{Event, JobId, JobState};
use omnidl::jobs::{Job, Jobs};
use omnidl::util;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// One server-sent event: its name and JSON payload.
#[derive(Clone, Debug)]
pub struct Msg {
    pub kind: &'static str,
    pub data: Arc<str>,
}

pub struct Model {
    pub jobs: Jobs,
    pub tools: DepsStatus,
    /// A complete tool check has come back; until then nothing counts as missing.
    pub tools_checked: bool,
    pub cfg: Config,
    /// The download folder was set with `--dir` and cannot be changed here.
    pub dir_locked: bool,
    seq: u64,
    tx: broadcast::Sender<Msg>,
}

impl Model {
    /// Sends a change to every open page, numbered.
    pub fn publish(&mut self, kind: &'static str, mut payload: Value) {
        self.seq += 1;
        payload["seq"] = json!(self.seq);
        // No receivers is fine: nobody has a page open.
        let _ = self.tx.send(Msg { kind, data: payload.to_string().into() });
    }

    pub fn settings(&self) -> Value {
        json!({ "settings": self.cfg, "dir_locked": self.dir_locked })
    }

    pub fn tools(&self) -> Value {
        let mut v = json!(self.tools);
        v["checked"] = json!(self.tools_checked);
        v
    }

    fn publish_job(&mut self, id: JobId) {
        if let Some(job) = self.jobs.get(id) {
            let view = job_view(&self.jobs, job);
            self.publish("job", json!({ "job": view }));
        }
    }
}

pub struct Hub {
    model: Mutex<Model>,
    tx: broadcast::Sender<Msg>,
}

impl Hub {
    pub fn new(cfg: Config, dir_locked: bool) -> Arc<Self> {
        let (tx, _) = broadcast::channel(1024);
        let model = Model {
            jobs: Jobs::default(),
            tools: DepsStatus::default(),
            tools_checked: false,
            cfg,
            dir_locked,
            seq: 0,
            tx: tx.clone(),
        };
        Arc::new(Self { model: Mutex::new(model), tx })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Msg> {
        self.tx.subscribe()
    }

    /// Works on the model; whatever `f` publishes goes out before the lock is released.
    pub fn with<R>(&self, f: impl FnOnce(&mut Model) -> R) -> R {
        f(&mut self.model.lock().unwrap())
    }

    /// The engine's sink.
    pub fn on_event(&self, ev: Event) {
        self.with(|m| match ev {
            Event::Job(id, update) => {
                m.jobs.apply(id, update);
                m.publish_job(id);
            }
            Event::Deps(d) => {
                // Progress messages while installing carry no versions; keep what is known.
                if d.ytdlp.is_none() && d.busy.is_some() {
                    m.tools.busy = d.busy;
                } else {
                    m.tools_checked |= d.busy.is_none();
                    m.tools = d;
                }
                let tools = m.tools();
                m.publish("tools", json!({ "tools": tools }));
            }
            // No window to bring forward, no self-update on servers.
            Event::Update(_) | Event::Remote { .. } => {}
        });
    }

    /// Everything a freshly opened page needs.
    pub fn snapshot(&self) -> Value {
        self.with(|m| {
            let jobs: Vec<Value> = m.jobs.list.iter().map(|j| job_view(&m.jobs, j)).collect();
            let mut v = m.settings();
            v["seq"] = json!(m.seq);
            v["jobs"] = json!(jobs);
            v["tools"] = m.tools();
            v
        })
    }
}

/// A job as the page shows it: the list model plus the German status line and
/// what can be done with it. Server paths stay on the server; only the file
/// name goes out.
pub fn job_view(jobs: &Jobs, job: &Job) -> Value {
    let child = job.parent.is_some();
    let (status, tone) = job.status(child);
    let mut v = json!(job);
    if let Some(obj) = v.as_object_mut() {
        obj.remove("output");
        obj.insert("status".into(), json!(status));
        obj.insert("tone".into(), json!(tone));
        let (download, size) = download_info(jobs, job);
        obj.insert("file".into(), json!(job.output.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy())));
        obj.insert("download".into(), json!(download));
        obj.insert("size".into(), json!(size.map(|b| util::fmt_bytes(b as f64))));
        obj.insert("can_retry".into(), json!(!child && job.state.can_retry() && !job.url.is_empty()));
    }
    v
}

/// "file", "folder" (photo posts) or "zip" (a finished playlist), with the
/// file size; `None` if there is nothing to fetch yet.
fn download_info(jobs: &Jobs, job: &Job) -> (Option<&'static str>, Option<u64>) {
    if let Some(path) = &job.output {
        return match std::fs::metadata(path) {
            Ok(m) if m.is_dir() => (Some("folder"), None),
            Ok(m) if m.is_file() => (Some("file"), Some(m.len())),
            _ => (None, None),
        };
    }
    let finished_group = job.is_group() && job.state.is_finished();
    ((finished_group && jobs.children_of(job.id).any(|c| c.output.is_some())).then_some("zip"), None)
}

/// The paths a job may hand out, read under the lock; the file system is
/// touched later, in [`offer`].
pub enum Target {
    Path { path: PathBuf },
    Group { title: String, files: Vec<PathBuf> },
}

pub fn target(jobs: &Jobs, id: JobId) -> Option<Target> {
    let job = jobs.get(id)?;
    if let Some(path) = &job.output {
        return Some(Target::Path { path: path.clone() });
    }
    if !job.is_group() || !job.state.is_finished() {
        return None;
    }
    let files: Vec<PathBuf> = jobs
        .children_of(id)
        .filter(|c| c.state == JobState::Done)
        .filter_map(|c| c.output.clone())
        .collect();
    (!files.is_empty()).then(|| Target::Group { title: job.title.clone(), files })
}

/// Turns a target into something to send: the file itself, or a ZIP of a
/// folder or of a playlist's files.
pub fn offer(target: Target) -> Option<Offer> {
    match target {
        Target::Path { path } if path.is_file() => Some(Offer::File(path)),
        Target::Path { path } if path.is_dir() => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "download".into());
            let name = util::sanitize_filename(&name);
            let entries: Vec<(String, PathBuf)> =
                files::walk(&path).into_iter().map(|(rel, p)| (format!("{name}/{rel}"), p)).collect();
            (!entries.is_empty()).then_some(Offer::Zip { name, entries })
        }
        Target::Path { .. } => None,
        Target::Group { title, files } => {
            let name = util::sanitize_filename(&title);
            let entries: Vec<(String, PathBuf)> = files
                .into_iter()
                .filter(|p| p.is_file())
                .filter_map(|p| {
                    let file = p.file_name()?.to_string_lossy().into_owned();
                    Some((format!("{name}/{file}"), p))
                })
                .collect();
            (!entries.is_empty()).then(|| Offer::Zip { name, entries: files::unique_names(entries) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnidl::job::JobUpdate;

    fn new(hub: &Hub, id: JobId, parent: Option<JobId>) {
        let url = format!("https://example.com/{id}");
        hub.on_event(Event::Job(id, JobUpdate::New { parent, url: url.clone(), title: url, source: "Web" }));
    }

    fn payload(msg: &Msg) -> Value {
        serde_json::from_str(&msg.data).unwrap()
    }

    #[test]
    fn events_are_numbered_and_the_snapshot_knows_the_last_number() {
        let hub = Hub::new(Config::default(), false);
        let mut rx = hub.subscribe();
        new(&hub, 1, None);
        hub.on_event(Event::Job(1, JobUpdate::State(JobState::Downloading)));
        let first = rx.try_recv().unwrap();
        let second = rx.try_recv().unwrap();
        assert_eq!((first.kind, second.kind), ("job", "job"));
        assert_eq!(payload(&first)["seq"], 1);
        assert_eq!(payload(&second)["seq"], 2);
        assert_eq!(payload(&second)["job"]["state"]["kind"], "downloading");

        let snap = hub.snapshot();
        assert_eq!(snap["seq"], 2);
        assert_eq!(snap["jobs"][0]["status"], "Web · Lädt …");
    }

    #[test]
    fn views_hide_server_paths() {
        let hub = Hub::new(Config::default(), false);
        new(&hub, 1, None);
        let dir = std::env::temp_dir().join("omnidl-tests").join("web-hub");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Video.mp4");
        std::fs::write(&file, b"x").unwrap();
        hub.on_event(Event::Job(1, JobUpdate::Output(file)));
        hub.on_event(Event::Job(1, JobUpdate::State(JobState::Done)));
        let job = &hub.snapshot()["jobs"][0];
        assert!(job.get("output").is_none());
        assert_eq!(job["file"], "Video.mp4");
        assert_eq!(job["download"], "file");
        assert_eq!(job["size"], "1 B");
        assert_eq!(job["can_retry"], false);
        assert_eq!(job["tone"], "normal");
    }

    #[test]
    fn busy_tool_messages_keep_the_versions() {
        let hub = Hub::new(Config::default(), false);
        let ready = DepsStatus { ytdlp: Some("2026.09.01".into()), ffmpeg: true, ..Default::default() };
        hub.on_event(Event::Deps(ready));
        hub.on_event(Event::Deps(DepsStatus { busy: Some("Lade yt-dlp …".into()), ..Default::default() }));
        let tools = &hub.snapshot()["tools"];
        assert_eq!(tools["ytdlp"], "2026.09.01");
        assert_eq!(tools["busy"], "Lade yt-dlp …");
        assert_eq!(tools["checked"], true);
    }

    #[test]
    fn finished_playlists_offer_their_files_as_zip() {
        let hub = Hub::new(Config::default(), false);
        new(&hub, 1, None);
        new(&hub, 2, Some(1));
        new(&hub, 3, Some(1));
        let dir = std::env::temp_dir().join("omnidl-tests").join("web-group");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("001 - a.mp3"), b"a").unwrap();
        hub.on_event(Event::Job(1, JobUpdate::Title("Meine: Liste".into())));
        hub.on_event(Event::Job(2, JobUpdate::Output(dir.join("001 - a.mp3"))));
        hub.on_event(Event::Job(2, JobUpdate::State(JobState::Done)));
        hub.on_event(Event::Job(3, JobUpdate::State(JobState::Failed("weg".into()))));
        assert!(hub.with(|m| target(&m.jobs, 1)).is_none(), "Gruppe läuft noch");
        hub.on_event(Event::Job(1, JobUpdate::State(JobState::Failed("1 von 2 fehlgeschlagen".into()))));

        assert_eq!(hub.snapshot()["jobs"][0]["download"], "zip");
        let offer = offer(hub.with(|m| target(&m.jobs, 1)).unwrap()).unwrap();
        let Offer::Zip { name, entries } = offer else { panic!("ZIP erwartet") };
        assert_eq!(name, "Meine_ Liste");
        assert_eq!(entries, vec![("Meine_ Liste/001 - a.mp3".to_string(), dir.join("001 - a.mp3"))]);
        assert!(hub.with(|m| target(&m.jobs, 3)).is_none(), "fehlgeschlagen: keine Datei");
        assert!(hub.with(|m| target(&m.jobs, 99)).is_none(), "unbekannt");
    }
}
