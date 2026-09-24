use crate::bridge::{self, Links};
use crate::config::Config;
use crate::deps::{self, DepsStatus, Tools};
use crate::detect::{self, Source, SpotifyKind};
use crate::job::{Event, JobId, JobState, JobUpdate};
use crate::journal::{Entry, Journal};
use crate::launch::Launch;
use crate::{gallery, matcher, peek, schedule, spotify, tagger, update, util, ytdlp};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore, watch};
use tokio_util::sync::CancellationToken;

pub enum Command {
    Add { urls: Vec<String>, cfg: Config, start_at: Option<i64> },
    Cancel(JobId),
    CancelAll,
    /// Starts a scheduled job right away.
    StartNow(JobId),
    SetParallel(usize),
    /// Settings used for links that arrive from outside the window.
    SetConfig(Config),
    UpdateTools,
    CheckUpdate { manual: bool },
    InstallUpdate(update::Release),
}

/// What the engine needs to start.
pub struct Start {
    pub ctx: egui::Context,
    pub base: PathBuf,
    pub cfg: Config,
    /// Port for the browser extension, if this instance got it.
    pub listener: Option<std::net::TcpListener>,
    /// Links omnidl was started with.
    pub launch: Launch,
}

#[derive(Clone)]
pub struct Shared {
    tx: std::sync::mpsc::Sender<Event>,
    ctx: egui::Context,
    pub tools: Arc<Tools>,
    sem: Arc<Semaphore>,
    parallel: Arc<Mutex<usize>>,
    next_id: Arc<AtomicU64>,
    cancels: Arc<Mutex<HashMap<JobId, CancellationToken>>>,
    wakes: Arc<Mutex<HashMap<JobId, Arc<Notify>>>>,
    journal: Arc<Journal>,
    /// yt-dlp and ffmpeg are in place; jobs wait for this.
    ready: watch::Sender<bool>,
    cfg: Arc<Mutex<Config>>,
    /// Look titles up before the download starts (off in tests: no network).
    peek_titles: bool,
}

impl Shared {
    fn new(
        tx: std::sync::mpsc::Sender<Event>,
        ctx: egui::Context,
        tools: Tools,
        journal: Journal,
        cfg: Config,
    ) -> Self {
        let parallel = cfg.parallel.clamp(1, 16);
        Self {
            tx,
            ctx,
            tools: Arc::new(tools),
            sem: Arc::new(Semaphore::new(parallel)),
            parallel: Arc::new(Mutex::new(parallel)),
            next_id: Arc::new(AtomicU64::new(1)),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            wakes: Arc::new(Mutex::new(HashMap::new())),
            journal: Arc::new(journal),
            ready: watch::Sender::new(false),
            cfg: Arc::new(Mutex::new(cfg)),
            peek_titles: true,
        }
    }

    fn send(&self, event: Event) {
        if self.tx.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }

    pub fn emit(&self, id: JobId, update: JobUpdate) {
        self.send(Event::Job(id, update));
    }

    fn emit_deps(&self, status: DepsStatus) {
        self.send(Event::Deps(status));
    }

    fn emit_update(&self, status: update::Status) {
        self.send(Event::Update(status));
    }

    fn new_job(&self, parent: Option<JobId>, url: String, title: String, source: &'static str) -> (JobId, CancellationToken) {
        self.register(parent, CancellationToken::new(), url, title, source)
    }

    /// Child of a group job: cancelling the group cancels it too, and its own
    /// cancel button only affects this one entry.
    fn new_child(
        &self,
        parent: JobId,
        parent_token: &CancellationToken,
        url: String,
        title: String,
        source: &'static str,
    ) -> (JobId, CancellationToken) {
        self.register(Some(parent), parent_token.child_token(), url, title, source)
    }

    fn register(
        &self,
        parent: Option<JobId>,
        token: CancellationToken,
        url: String,
        title: String,
        source: &'static str,
    ) -> (JobId, CancellationToken) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.cancels.lock().unwrap().insert(id, token.clone());
        self.emit(id, JobUpdate::New { parent, url, title, source });
        (id, token)
    }

    /// Final state. Top-level jobs leave the journal: nothing to resume.
    fn finish(&self, id: JobId, state: JobState) {
        self.cancels.lock().unwrap().remove(&id);
        self.journal.remove(id);
        self.emit(id, JobUpdate::State(state));
    }

    fn cancel(&self, id: JobId) {
        if let Some(t) = self.cancels.lock().unwrap().get(&id) {
            t.cancel();
        }
    }

    fn cancel_all(&self) {
        for t in self.cancels.lock().unwrap().values() {
            t.cancel();
        }
    }

    fn start_now(&self, id: JobId) {
        if let Some(w) = self.wakes.lock().unwrap().get(&id) {
            w.notify_one();
        }
    }

    fn set_parallel(&self, n: usize) {
        let n = n.clamp(1, 16);
        let mut cur = self.parallel.lock().unwrap();
        if n > *cur {
            self.sem.add_permits(n - *cur);
        } else if n < *cur {
            self.sem.forget_permits(*cur - n);
        }
        *cur = n;
    }

    /// Links from the browser extension or a second start of the app, added
    /// with the settings currently shown in the window.
    fn add_links(&self, links: Links) {
        let cfg = self.cfg.lock().unwrap().for_mode(links.mode);
        for url in links.urls {
            submit(self, Entry::new(url, cfg.clone(), None));
        }
        self.send(Event::Remote { focus: links.focus });
    }
}

pub fn start(s: Start) -> (tokio::sync::mpsc::UnboundedSender<Command>, std::sync::mpsc::Receiver<Event>) {
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    let (ev_tx, ev_rx) = std::sync::mpsc::channel::<Event>();

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ev_tx.send(Event::Deps(DepsStatus {
                    error: Some(format!("Laufzeitumgebung nicht startbar: {e}")),
                    ..Default::default()
                }));
                return;
            }
        };
        rt.block_on(async move {
            let (journal, leftovers) = Journal::open(s.base.join("queue.json"));
            let shared = Shared::new(ev_tx, s.ctx, Tools::new(&s.base), journal, s.cfg);

            // Re-add first: the journal file still holds these until they are.
            for entry in leftovers {
                submit(&shared, entry);
            }
            if !s.launch.urls.is_empty() {
                let links = Links { urls: s.launch.urls, mode: s.launch.mode, focus: false };
                shared.add_links(links);
            }
            if let Some(listener) = s.listener {
                serve_bridge(&shared, listener);
            }
            {
                let s = shared.clone();
                tokio::spawn(async move { prepare_tools(&s, false).await });
            }
            {
                let s = shared.clone();
                tokio::spawn(async move { update_loop(s).await });
            }

            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    Command::Add { urls, cfg, start_at } => {
                        for url in urls {
                            submit(&shared, Entry::new(url, cfg.clone(), start_at));
                        }
                    }
                    Command::Cancel(id) => shared.cancel(id),
                    Command::CancelAll => shared.cancel_all(),
                    Command::StartNow(id) => shared.start_now(id),
                    Command::SetParallel(n) => shared.set_parallel(n),
                    Command::SetConfig(cfg) => *shared.cfg.lock().unwrap() = cfg,
                    Command::UpdateTools => {
                        let s = shared.clone();
                        tokio::spawn(async move { prepare_tools(&s, true).await });
                    }
                    Command::CheckUpdate { manual } => {
                        let s = shared.clone();
                        tokio::spawn(async move { check_update(&s, manual).await });
                    }
                    Command::InstallUpdate(rel) => {
                        let s = shared.clone();
                        tokio::spawn(async move { install_update(&s, rel).await });
                    }
                }
            }
        });
    });

    (cmd_tx, ev_rx)
}

fn serve_bridge(shared: &Shared, listener: std::net::TcpListener) {
    let listener = listener.set_nonblocking(true).and_then(|_| tokio::net::TcpListener::from_std(listener));
    let Ok(listener) = listener else { return };
    let s = shared.clone();
    tokio::spawn(bridge::serve(listener, bridge::PORT, Arc::new(move |links| s.add_links(links))));
}

async fn prepare_tools(shared: &Shared, force: bool) {
    if force {
        // yt-dlp is about to be replaced; new jobs wait for it.
        shared.ready.send_replace(false);
    }
    let mut status = deps::status(&shared.tools).await;
    status.busy = Some("Prüfe Tools …".into());
    shared.emit_deps(status);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let reporter = {
        let shared = shared.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                shared.emit_deps(DepsStatus { busy: Some(msg), ..Default::default() });
            }
        })
    };
    let result = deps::ensure(&shared.tools, force, |msg| {
        let _ = tx.send(msg);
    })
    .await;
    drop(tx);
    let _ = reporter.await;

    let mut status = deps::status(&shared.tools).await;
    if let Err(e) = result {
        status.error = Some(format!("{e:#}"));
    }
    shared.ready.send_replace(status.ready());
    shared.emit_deps(status);
}

// ---------------------------------------------------------------- app updates

async fn update_loop(shared: Shared) {
    tokio::time::sleep(Duration::from_secs(8)).await;
    loop {
        let wanted = shared.cfg.lock().unwrap().check_updates;
        if wanted && update::auto_enabled() {
            check_update(&shared, false).await;
        }
        tokio::time::sleep(Duration::from_secs(12 * 3600)).await;
    }
}

/// Automatic checks stay silent unless there is something new.
async fn check_update(shared: &Shared, manual: bool) {
    if manual {
        shared.emit_update(update::Status::Checking);
    }
    match update::check().await {
        Ok(Some(rel)) => shared.emit_update(update::Status::Available(rel)),
        Ok(None) if manual => shared.emit_update(update::Status::UpToDate),
        Err(e) if manual => shared.emit_update(update::Status::Failed(format!("{e:#}"))),
        _ => {}
    }
}

async fn install_update(shared: &Shared, rel: update::Release) {
    let version = rel.version.clone();
    let downloading = |frac| update::Status::Downloading { version: version.clone(), frac };
    shared.emit_update(downloading(None));
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return shared.emit_update(update::Status::Failed(e.to_string())),
    };
    let result = update::install(&rel, &exe, |frac| shared.emit_update(downloading(frac))).await;
    shared.emit_update(match result {
        Ok(()) => update::Status::Ready(rel.version),
        Err(e) => update::Status::Failed(format!("{e:#}")),
    });
}

// ---------------------------------------------------------------- jobs

/// Adds one link as a top-level job and records it in the journal.
fn submit(shared: &Shared, entry: Entry) {
    let source = detect::detect(&entry.url).label();
    let (id, token) = shared.new_job(None, entry.url.clone(), entry.url.clone(), source);
    shared.journal.insert(id, entry.clone());
    // Scheduled and waiting jobs show the real name, not the bare link.
    if shared.peek_titles && peek::endpoint(&entry.url).is_some() {
        let s = shared.clone();
        let url = entry.url.clone();
        tokio::spawn(async move {
            if let Some(title) = peek::title(&url).await {
                s.emit(id, JobUpdate::Title(title));
            }
        });
    }
    let s = shared.clone();
    tokio::spawn(async move { run(s, id, token, entry).await });
}

async fn run(shared: Shared, id: JobId, token: CancellationToken, entry: Entry) {
    if let Some(at) = entry.start_at {
        if !wait_until(&shared, id, at, &token).await {
            shared.finish(id, JobState::Cancelled);
            return;
        }
        shared.journal.started(id);
    }
    if !wait_for_tools(&shared, id, &token).await {
        shared.finish(id, JobState::Cancelled);
        return;
    }
    let done: HashSet<String> = entry.done.into_iter().collect();
    handle_url(shared, id, token, entry.url, entry.cfg, done).await;
}

/// Sleeps until the start time; `false` if cancelled first. Checks the wall
/// clock in short steps so a computer waking from sleep starts on time.
async fn wait_until(shared: &Shared, id: JobId, at: i64, token: &CancellationToken) -> bool {
    let wake = Arc::new(Notify::new());
    shared.wakes.lock().unwrap().insert(id, wake.clone());
    shared.emit(id, JobUpdate::State(JobState::Scheduled(at)));
    let started = loop {
        let left = at - schedule::now_unix();
        if left <= 0 {
            break true;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(left.min(30) as u64)) => {}
            _ = wake.notified() => break true,
            _ = token.cancelled() => break false,
        }
    };
    shared.wakes.lock().unwrap().remove(&id);
    started
}

/// Links can be added while the tools are still being set up; they wait here.
async fn wait_for_tools(shared: &Shared, id: JobId, token: &CancellationToken) -> bool {
    let mut ready = shared.ready.subscribe();
    if *ready.borrow() {
        return true;
    }
    shared.emit(id, JobUpdate::State(JobState::Queued));
    shared.emit(id, JobUpdate::Note("Wartet auf die Werkzeuge …".into()));
    let ok = tokio::select! {
        r = ready.wait_for(|r| *r) => r.is_ok(),
        _ = token.cancelled() => false,
    };
    shared.emit(id, JobUpdate::Note(String::new()));
    ok
}

/// Dispatches one link. `done` lists list items finished in an earlier session.
async fn handle_url(shared: Shared, id: JobId, token: CancellationToken, url: String, cfg: Config, done: HashSet<String>) {
    match detect::detect(&url) {
        Source::Spotify(kind, sid) => spotify_job(shared, id, token, kind, sid, cfg, done).await,
        Source::SpotifyShort => match resolve_redirect(&url).await {
            Some(real) => match detect::detect(&real) {
                Source::Spotify(kind, sid) => spotify_job(shared, id, token, kind, sid, cfg, done).await,
                _ => single_job(shared, id, token, real, cfg).await,
            },
            None => shared.finish(id, JobState::Failed("Kurzlink nicht auflösbar".into())),
        },
        ref s if s.is_list() && cfg.playlist => list_job(shared, id, token, url, cfg, done).await,
        _ => single_job(shared, id, token, url, cfg).await,
    }
}

async fn resolve_redirect(url: &str) -> Option<String> {
    let resp = util::http().get(url).send().await.ok()?;
    Some(resp.url().to_string())
}

/// One URL -> one file (or one yt-dlp managed playlist).
async fn single_job(shared: Shared, id: JobId, token: CancellationToken, url: String, cfg: Config) {
    let Some(_permit) = acquire(&shared, id, &token).await else {
        shared.finish(id, JobState::Cancelled);
        return;
    };

    let is_photo = matches!(detect::detect(&url), Source::TikTokPhoto);
    if is_photo {
        run_gallery(&shared, id, &url, &cfg, &token).await;
        return;
    }

    shared.emit(id, JobUpdate::State(JobState::Downloading));
    let req = ytdlp::Request {
        url: &url,
        dir: &cfg.download_dir,
        template: "%(title).180B.%(ext)s".into(),
        cfg: &cfg,
        playlist: cfg.playlist,
        embed_meta: true,
        force_audio: false,
        duration: None,
    };
    let out = ytdlp::run(&shared, id, &shared.tools, &req, &token).await;
    if out.cancelled {
        shared.finish(id, JobState::Cancelled);
        return;
    }
    if let Some(file) = out.files.last() {
        shared.emit(id, JobUpdate::Output(file.clone()));
        shared.finish(id, JobState::Done);
        return;
    }
    // yt-dlp only handles video. Photo posts fall back to gallery-dl.
    let err = out.error.unwrap_or_else(|| "unbekannter Fehler".into());
    if looks_like_photo_post(&err) {
        run_gallery(&shared, id, &url, &cfg, &token).await;
    } else {
        shared.finish(id, JobState::Failed(err));
    }
}

fn looks_like_photo_post(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("no video formats")
        || e.contains("unsupported url")
        || e.contains("no video could be found")
        || e.contains("there's no video")
}

async fn run_gallery(shared: &Shared, id: JobId, url: &str, cfg: &Config, token: &CancellationToken) {
    shared.emit(id, JobUpdate::State(JobState::Downloading));
    match gallery::download(
        shared,
        id,
        &shared.tools,
        url,
        &cfg.download_dir,
        cfg.cookies.browser(),
        token,
    )
    .await
    {
        Ok(dir) => {
            shared.emit(id, JobUpdate::Output(dir));
            shared.finish(id, JobState::Done);
        }
        Err(e) if token.is_cancelled() => {
            let _ = e;
            shared.finish(id, JobState::Cancelled);
        }
        Err(e) => shared.finish(id, JobState::Failed(e)),
    }
}

/// Playlist / channel: expanded into one job per entry so they download in parallel.
async fn list_job(shared: Shared, group: JobId, token: CancellationToken, url: String, cfg: Config, done: HashSet<String>) {
    shared.emit(group, JobUpdate::State(JobState::Resolving));

    let listed = ytdlp::flat_list(&shared.tools, &url, None, cfg.cookies.browser()).await;
    let (title, entries) = match listed {
        Ok(v) => v,
        Err(e) => {
            shared.finish(group, JobState::Failed(format!("{e:#}")));
            return;
        }
    };
    if entries.is_empty() {
        shared.finish(group, JobState::Failed("Liste ist leer".into()));
        return;
    }
    let name = title.unwrap_or_else(|| "Playlist".into());
    shared.emit(group, JobUpdate::Title(name.clone()));
    shared.emit(group, JobUpdate::State(JobState::Group));

    let dir = cfg.download_dir.join(util::sanitize_filename(&name));
    let counter = Arc::new(Counter::new(group, entries.len()));
    counter.resume(&shared, entries.iter().filter(|e| done.contains(&e.url)).count());
    let mut tasks = Vec::new();
    for (i, entry) in entries.into_iter().enumerate() {
        if done.contains(&entry.url) {
            continue;
        }
        let shared = shared.clone();
        let cfg = cfg.clone();
        let dir = dir.clone();
        let counter = counter.clone();
        let gtoken = token.clone();
        tasks.push(tokio::spawn(async move {
            let (id, token) = shared.new_child(group, &gtoken, entry.url.clone(), entry.title.clone(), "Titel");
            let state = match acquire(&shared, id, &token).await {
                Some(_permit) => {
                    shared.emit(id, JobUpdate::State(JobState::Downloading));
                    let req = ytdlp::Request {
                        url: &entry.url,
                        dir: &dir,
                        template: format!("{:03} - %(title).150B.%(ext)s", i + 1),
                        cfg: &cfg,
                        playlist: false,
                        embed_meta: true,
                        force_audio: false,
                        duration: None,
                    };
                    let out = ytdlp::run(&shared, id, &shared.tools, &req, &token).await;
                    if out.cancelled {
                        JobState::Cancelled
                    } else if let Some(f) = out.files.last() {
                        shared.emit(id, JobUpdate::Output(f.clone()));
                        JobState::Done
                    } else {
                        JobState::Failed(out.error.unwrap_or_else(|| "fehlgeschlagen".into()))
                    }
                }
                None => JobState::Cancelled,
            };
            let ok = matches!(state, JobState::Done);
            if ok {
                shared.journal.mark_done(group, &entry.url);
            }
            shared.finish(id, state);
            counter.record(&shared, ok);
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
    shared.finish(group, counter.final_state());
}

/// Identifies a Spotify track across sessions.
fn track_key(track: &spotify::Track) -> String {
    if track.id.is_empty() { format!("{} - {}", track.artist_line(), track.title) } else { track.id.clone() }
}

/// Spotify: read the track list, then find and download each song elsewhere.
async fn spotify_job(
    shared: Shared,
    group: JobId,
    token: CancellationToken,
    kind: SpotifyKind,
    sid: String,
    cfg: Config,
    done: HashSet<String>,
) {
    shared.emit(group, JobUpdate::State(JobState::Resolving));

    let coll = match spotify::fetch(kind, &sid).await {
        Ok(c) => c,
        Err(e) => {
            shared.finish(group, JobState::Failed(format!("{e:#}")));
            return;
        }
    };
    if coll.tracks.is_empty() {
        shared.finish(group, JobState::Failed("keine Titel gefunden".into()));
        return;
    }
    shared.emit(group, JobUpdate::Title(coll.name.clone()));

    // A single track needs no group.
    if coll.kind == "track" {
        let track = coll.tracks.into_iter().next().unwrap();
        let dir = cfg.download_dir.clone();
        let state = match acquire(&shared, group, &token).await {
            Some(_permit) => spotify_track(&shared, group, &track, &dir, &cfg, &token, false).await,
            None => JobState::Cancelled,
        };
        shared.finish(group, state);
        return;
    }

    if coll.truncated {
        shared.emit(
            group,
            JobUpdate::Note("nur die ersten 100 Titel (Spotify-Limit)".into()),
        );
    }
    shared.emit(group, JobUpdate::State(JobState::Group));

    let dir = cfg.download_dir.join(util::sanitize_filename(&coll.name));
    let counter = Arc::new(Counter::new(group, coll.tracks.len()));
    counter.resume(&shared, coll.tracks.iter().filter(|t| done.contains(&track_key(t))).count());
    let mut tasks = Vec::new();
    for mut track in coll.tracks {
        let key = track_key(&track);
        if done.contains(&key) {
            continue;
        }
        // Playlist entries carry no artwork; the list cover is the fallback.
        if track.cover.is_none() {
            track.cover = coll.cover.clone();
        }
        let shared = shared.clone();
        let cfg = cfg.clone();
        let dir = dir.clone();
        let counter = counter.clone();
        let gtoken = token.clone();
        tasks.push(tokio::spawn(async move {
            let label = format!("{} – {}", track.artist_line(), track.title);
            let url = format!("https://open.spotify.com/track/{}", track.id);
            let (id, token) = shared.new_child(group, &gtoken, url, label, "Spotify");
            let state = match acquire(&shared, id, &token).await {
                Some(_permit) => spotify_track(&shared, id, &track, &dir, &cfg, &token, true).await,
                None => JobState::Cancelled,
            };
            let ok = matches!(state, JobState::Done);
            if ok {
                shared.journal.mark_done(group, &key);
            }
            shared.finish(id, state);
            counter.record(&shared, ok);
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
    shared.finish(group, counter.final_state());
}

/// Finds the best source for one Spotify track and downloads it.
async fn spotify_track(
    shared: &Shared,
    id: JobId,
    track: &spotify::Track,
    dir: &Path,
    cfg: &Config,
    token: &CancellationToken,
    numbered: bool,
) -> JobState {
    shared.emit(id, JobUpdate::State(JobState::Resolving));

    // The list view has no cover art and splits artist names on commas.
    let mut track = track.clone();
    if !track.id.is_empty() {
        if let Ok(full) = spotify::track_details(&track.id).await {
            track.cover = full.cover.or(track.cover);
            if !full.artists.is_empty() {
                track.artists = full.artists;
            }
        }
    }

    let candidates = matcher::search(&shared.tools, &track, cfg.cookies.browser()).await;
    if token.is_cancelled() {
        return JobState::Cancelled;
    }
    if candidates.is_empty() {
        return JobState::NoMatch;
    }

    let stem = util::sanitize_filename(&format!("{} - {}", track.artist_line(), track.title));
    let name = match (numbered, track.number) {
        (true, Some(n)) => format!("{n:03} - {stem}"),
        _ => stem,
    };
    let template = format!("{}.%(ext)s", util::escape_template(&name));
    let window = (track.duration_s * 0.07).max(10.0);
    let duration = (track.duration_s > 0.0)
        .then(|| (track.duration_s - window, track.duration_s + window));

    shared.emit(id, JobUpdate::State(JobState::Downloading));
    for cand in candidates.iter().take(3) {
        let req = ytdlp::Request {
            url: &cand.url,
            dir,
            template: template.clone(),
            cfg,
            playlist: false,
            embed_meta: false,
            force_audio: true,
            duration,
        };
        let out = ytdlp::run(shared, id, &shared.tools, &req, token).await;
        if out.cancelled {
            return JobState::Cancelled;
        }
        if let Some(file) = out.files.last() {
            if let Err(e) = tagger::tag(file, &track).await {
                shared.emit(id, JobUpdate::Note(format!("Tags unvollständig: {e}")));
            }
            shared.emit(id, JobUpdate::Output(file.clone()));
            return JobState::Done;
        }
        // Wrong length or unavailable -> try the next candidate.
    }
    JobState::NoMatch
}

/// Progress bookkeeping for a group job.
struct Counter {
    group: JobId,
    total: usize,
    done: AtomicUsize,
    failed: AtomicUsize,
}

impl Counter {
    fn new(group: JobId, total: usize) -> Self {
        Self { group, total, done: AtomicUsize::new(0), failed: AtomicUsize::new(0) }
    }

    /// Items already finished in an earlier session count as done.
    fn resume(&self, shared: &Shared, already: usize) {
        if already > 0 {
            self.done.fetch_add(already, Ordering::Relaxed);
            self.report(shared);
        }
    }

    fn record(&self, shared: &Shared, ok: bool) {
        if ok {
            self.done.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed.fetch_add(1, Ordering::Relaxed);
        }
        self.report(shared);
    }

    fn report(&self, shared: &Shared) {
        let done = self.done.load(Ordering::Relaxed) + self.failed.load(Ordering::Relaxed);
        shared.emit(
            self.group,
            JobUpdate::Item { index: done as u32, count: self.total as u32 },
        );
        shared.emit(
            self.group,
            JobUpdate::Progress {
                frac: Some(done as f32 / self.total.max(1) as f32),
                speed: None,
                eta: None,
            },
        );
    }

    fn final_state(&self) -> JobState {
        let failed = self.failed.load(Ordering::Relaxed);
        if failed == 0 {
            JobState::Done
        } else if self.done.load(Ordering::Relaxed) == 0 {
            JobState::Failed(format!("alle {failed} fehlgeschlagen"))
        } else {
            JobState::Failed(format!("{failed} von {} fehlgeschlagen", self.total))
        }
    }
}

/// Waits for a worker slot; `None` if the job was cancelled while queued.
/// The caller reports the resulting state.
async fn acquire(
    shared: &Shared,
    id: JobId,
    token: &CancellationToken,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    shared.emit(id, JobUpdate::State(JobState::Queued));
    tokio::select! {
        permit = shared.sem.clone().acquire_owned() => permit.ok(),
        _ = token.cancelled() => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Format, Mode};
    use std::sync::mpsc::Receiver;
    use std::time::Instant;

    /// Engine wired to a headless egui context, using the repo's `bin/` tools.
    fn test_env(sub: &str) -> (Shared, Receiver<Event>, Config) {
        let dir = std::env::temp_dir().join("omnidl-tests").join(sub);
        let _ = std::fs::remove_dir_all(&dir);
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = Config { download_dir: dir, parallel: 4, ..Config::default() };
        let tools = Tools::new(Path::new(env!("CARGO_MANIFEST_DIR")));
        let mut shared = Shared::new(tx, egui::Context::default(), tools, Journal::in_memory(), cfg.clone());
        shared.peek_titles = false;
        (shared, rx, cfg)
    }

    /// Same, with the tools marked ready as after the startup check.
    fn ready_env(sub: &str) -> (Shared, Receiver<Event>, Config) {
        let env = test_env(sub);
        env.0.ready.send_replace(true);
        env
    }

    /// Last state per job plus every produced output path.
    fn collect(rx: Receiver<Event>) -> (HashMap<JobId, JobState>, Vec<PathBuf>) {
        let mut states = HashMap::new();
        let mut files = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let Event::Job(id, update) = ev {
                match update {
                    JobUpdate::State(s) => {
                        states.insert(id, s);
                    }
                    JobUpdate::Output(p) => files.push(p),
                    _ => {}
                }
            }
        }
        (states, files)
    }

    /// Reads job updates until `want` matches one, or panics after a few seconds.
    async fn wait_for(rx: &Receiver<Event>, mut want: impl FnMut(JobId, &JobUpdate) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            while let Ok(ev) = rx.try_recv() {
                if let Event::Job(id, u) = &ev {
                    if want(*id, u) {
                        return;
                    }
                }
            }
            assert!(Instant::now() < deadline, "erwartetes Ereignis kam nicht");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn later() -> i64 {
        schedule::now_unix() + 3600
    }

    #[tokio::test]
    async fn scheduled_job_waits_and_can_be_cancelled() {
        let (shared, rx, cfg) = test_env("sched-cancel");
        submit(&shared, Entry::new("https://youtu.be/x".into(), cfg, Some(later())));
        wait_for(&rx, |id, u| id == 1 && matches!(u, JobUpdate::State(JobState::Scheduled(_)))).await;
        assert_eq!(shared.journal.len(), 1, "geplant heißt: steht im Journal");

        shared.cancel(1);
        wait_for(&rx, |id, u| id == 1 && matches!(u, JobUpdate::State(JobState::Cancelled))).await;
        assert_eq!(shared.journal.len(), 0, "abgebrochen: nichts mehr fortzusetzen");
    }

    #[tokio::test]
    async fn start_now_skips_the_wait() {
        let (shared, rx, cfg) = test_env("sched-now");
        submit(&shared, Entry::new("https://youtu.be/x".into(), cfg, Some(later())));
        wait_for(&rx, |_, u| matches!(u, JobUpdate::State(JobState::Scheduled(_)))).await;

        shared.start_now(1);
        // Tools are not ready in this setup, so the job moves on to waiting for them.
        wait_for(&rx, |_, u| matches!(u, JobUpdate::Note(n) if n.contains("Werkzeuge"))).await;
        assert_eq!(shared.journal.get(1).unwrap().start_at, None, "Neustart startet sofort");
        shared.cancel_all();
        wait_for(&rx, |_, u| matches!(u, JobUpdate::State(JobState::Cancelled))).await;
    }

    #[tokio::test]
    async fn jobs_wait_for_the_tools() {
        let (shared, rx, cfg) = test_env("tools-gate");
        submit(&shared, Entry::new("https://youtu.be/x".into(), cfg, None));
        wait_for(&rx, |_, u| matches!(u, JobUpdate::Note(n) if n.contains("Werkzeuge"))).await;
        let mut ready = shared.ready.subscribe();
        assert!(!*ready.borrow_and_update());
        shared.cancel(1);
        wait_for(&rx, |_, u| matches!(u, JobUpdate::State(JobState::Cancelled))).await;
    }

    #[tokio::test]
    async fn links_from_outside_use_the_window_settings() {
        let (shared, rx, mut cfg) = test_env("remote");
        cfg.set_format(Format::Flac);
        cfg.set_audio(false);
        *shared.cfg.lock().unwrap() = cfg;
        shared.add_links(Links { urls: vec!["https://youtu.be/a".into()], mode: Some(Mode::Audio), focus: false });

        let entry = shared.journal.get(1).expect("im Journal");
        assert_eq!(entry.cfg.format, Format::Flac, "Audio im zuletzt gewählten Format");
        let mut remote = None;
        let mut new_url = None;
        for ev in rx.try_iter() {
            match ev {
                Event::Remote { focus } => remote = Some(focus),
                Event::Job(1, JobUpdate::New { url, parent: None, .. }) => new_url = Some(url),
                _ => {}
            }
        }
        assert_eq!(remote, Some(false), "aus dem Browser: Fenster bleibt im Hintergrund");
        assert_eq!(new_url.as_deref(), Some("https://youtu.be/a"));
        shared.cancel_all();
    }

    #[tokio::test]
    async fn resumed_groups_count_finished_items() {
        let (shared, rx, _) = test_env("resume-count");
        let c = Counter::new(7, 10);
        c.resume(&shared, 4);
        c.resume(&shared, 0);
        c.record(&shared, true);
        let items: Vec<(u32, u32)> = rx
            .try_iter()
            .filter_map(|e| match e {
                Event::Job(7, JobUpdate::Item { index, count }) => Some((index, count)),
                _ => None,
            })
            .collect();
        assert_eq!(items, vec![(4, 10), (5, 10)]);
    }

    #[test]
    fn spotify_tracks_have_stable_keys() {
        let mut t = spotify::Track { id: "abc".into(), title: "Song".into(), artists: vec!["A".into()], ..Default::default() };
        assert_eq!(track_key(&t), "abc");
        t.id.clear();
        assert_eq!(track_key(&t), "A - Song");
    }

    #[tokio::test]
    #[ignore = "braucht Netzwerk"]
    async fn e2e_youtube_to_mp3() {
        let (shared, rx, mut cfg) = ready_env("yt");
        cfg.format = Format::Mp3;
        cfg.playlist = false;
        let (id, token) = shared.new_job(None, String::new(), String::new(), "YouTube");
        handle_url(shared, id, token, "https://www.youtube.com/watch?v=jNQXAC9IVRw".into(), cfg, HashSet::new()).await;
        let (states, files) = collect(rx);
        assert_eq!(states.get(&1), Some(&JobState::Done), "Status: {states:?}");
        assert_eq!(files.len(), 1, "genau eine Datei");
        assert_eq!(files[0].extension().unwrap(), "mp3");
        assert!(files[0].exists(), "Datei liegt wirklich da");
    }

    /// Die playlist-spezifische Logik ist das Auffächern in Einträge; der
    /// Download-Weg selbst ist durch die anderen e2e-Tests abgedeckt. Ein echter
    /// Playlist-Download wäre hier stundenlanges Videomaterial.
    #[tokio::test]
    #[ignore = "braucht Netzwerk"]
    async fn e2e_playlist_resolves_into_entries() {
        let tools = Tools::new(Path::new(env!("CARGO_MANIFEST_DIR")));
        let (title, entries) = ytdlp::flat_list(
            &tools,
            "https://www.youtube.com/playlist?list=PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf",
            None,
            None,
        )
        .await
        .expect("Playlist lesbar");
        assert_eq!(title.as_deref(), Some("Select Lectures"));
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.url.starts_with("https://")));
        assert!(entries.iter().all(|e| e.duration.is_some_and(|d| d > 0.0)));
        assert!(entries.iter().all(|e| !e.title.is_empty()));
    }

    #[tokio::test]
    #[ignore = "braucht Netzwerk"]
    async fn e2e_gallery_dl_downloads_images() {
        let (shared, _rx, cfg) = ready_env("gd");
        let (id, token) = shared.new_job(None, String::new(), "test".into(), "Web");
        let result = gallery::download(
            &shared,
            id,
            &shared.tools,
            "https://commons.wikimedia.org/wiki/File:Example.jpg",
            &cfg.download_dir,
            None,
            &token,
        )
        .await;
        let dir = match result {
            Ok(dir) => dir,
            // Wiederholte Läufe laufen in die Drosselung der Gegenstelle. Das sagt
            // nichts über den Code aus, also kein Fehlschlag.
            Err(e) if e.contains("429") || e.to_lowercase().contains("too many requests") => {
                eprintln!("übersprungen: Gegenstelle drosselt ({e})");
                return;
            }
            Err(e) => panic!("gallery-dl fehlgeschlagen: {e}"),
        };
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
        assert!(!files.is_empty(), "Bilder liegen in {dir:?}");
        assert!(
            files.iter().all(|f| f.metadata().unwrap().len() > 1_000),
            "keine leeren Dateien"
        );
    }

    #[tokio::test]
    #[ignore = "braucht Netzwerk"]
    async fn e2e_spotify_track_is_matched_and_tagged() {
        let (shared, rx, mut cfg) = ready_env("sp");
        cfg.format = Format::Mp3;
        submit(&shared, Entry::new("https://open.spotify.com/track/4PTG3Z6ehGkBFwjybzWkR8".into(), cfg, None));
        wait_for(&rx, |id, u| id == 1 && matches!(u, JobUpdate::State(s) if s.is_finished())).await;
        let (_, files) = collect(rx);
        let file = files.first().expect("eine Datei");
        assert!(file.exists());
        let tagged = lofty::probe::Probe::open(file).unwrap().read().unwrap();
        let tag = lofty::file::TaggedFileExt::primary_tag(&tagged).expect("Tags vorhanden");
        assert!(lofty::prelude::Accessor::title(tag).is_some(), "Titel gesetzt");
        assert!(!tag.pictures().is_empty(), "Cover eingebettet");
    }

    /// Echte yt-dlp-Fehlertexte, die den Wechsel auf gallery-dl auslösen müssen.
    #[test]
    fn photo_posts_trigger_the_gallery_fallback() {
        assert!(looks_like_photo_post("No video formats found!; please report this issue"));
        assert!(looks_like_photo_post("Unsupported URL: https://example.com/x"));
        assert!(looks_like_photo_post("There's no video in this post"));
        // Echte Fehler dürfen nicht umgeleitet werden
        assert!(!looks_like_photo_post("Video unavailable. This video is private"));
        assert!(!looks_like_photo_post("HTTP Error 429: Too Many Requests"));
        assert!(!looks_like_photo_post("Requested post not available"));
    }

    #[test]
    fn group_counter_tracks_progress_and_outcome() {
        let (shared, rx, _) = test_env("counter");
        let c = Counter::new(1, 3);
        c.record(&shared, true);
        c.record(&shared, false);
        assert_eq!(c.final_state(), JobState::Failed("1 von 3 fehlgeschlagen".into()));
        c.record(&shared, true);
        assert_eq!(c.final_state(), JobState::Failed("1 von 3 fehlgeschlagen".into()));

        let items: Vec<(u32, u32)> = rx
            .try_iter()
            .filter_map(|e| match e {
                Event::Job(_, JobUpdate::Item { index, count }) => Some((index, count)),
                _ => None,
            })
            .collect();
        assert_eq!(items, vec![(1, 3), (2, 3), (3, 3)], "zählt jeden Abschluss");

        let all_ok = Counter::new(1, 2);
        all_ok.record(&shared, true);
        all_ok.record(&shared, true);
        assert_eq!(all_ok.final_state(), JobState::Done);
    }
}
