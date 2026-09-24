//! Routes: the page, the JSON API and the event stream, behind one guard that
//! handles login, cross-site protection and security headers.

use super::auth::{Auth, Login};
use super::hub::{self, Hub};
use super::{files, net};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use omnidl::config::{AudioQuality, Config, Cookies, Format, Mode, VideoQuality};
use omnidl::engine::Command;
use omnidl::job::JobId;
use omnidl::{detect, launch, schedule, update};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

/// Header every state-changing request must carry. Browsers cannot add it to
/// cross-site requests without a CORS preflight, which this server never grants.
pub const CSRF_HEADER: &str = "x-omnidl";
/// Links per request; more is surely a mistake.
const MAX_LINKS: usize = 500;

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                   connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// Headers set by reverse proxies. Without a password only direct local access is allowed.
const FORWARDED: [&str; 4] = ["forwarded", "x-forwarded-for", "x-forwarded-host", "x-real-ip"];

/// Reachable without logging in: the login page and what it needs.
const PUBLIC: [&str; 6] = ["/login", "/api/login", "/app.css", "/login.js", "/icon.png", "/manifest.webmanifest"];

pub struct App {
    pub hub: Arc<Hub>,
    pub cmd: UnboundedSender<Command>,
    pub auth: Auth,
    /// With `--dir`: the folder from `config.toml`, written back instead of the override.
    pub stored_dir: Option<PathBuf>,
    /// Write `config.toml` on changes (off in tests).
    pub save_config: bool,
    /// Ends open event streams so the server can stop.
    pub shutdown: CancellationToken,
}

impl App {
    fn send(&self, c: Command) -> Result<(), Response> {
        self.cmd
            .send(c)
            .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Der Download-Dienst läuft nicht mehr."))
    }
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(|| async { asset(INDEX_HTML, "text/html; charset=utf-8") }))
        .route("/login", get(login_page))
        .route("/app.css", get(|| async { asset(APP_CSS, "text/css; charset=utf-8") }))
        .route("/app.js", get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }))
        .route("/login.js", get(|| async { asset(LOGIN_JS, "text/javascript; charset=utf-8") }))
        .route("/icon.png", get(|| async { asset(ICON_PNG, "image/png") }))
        .route("/manifest.webmanifest", get(|| async { asset(MANIFEST, "application/manifest+json") }))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/state", get(state))
        .route("/api/events", get(events))
        .route("/api/add", post(add))
        .route("/api/jobs/{id}/cancel", post(cancel))
        .route("/api/jobs/{id}/start-now", post(start_now))
        .route("/api/jobs/{id}/retry", post(retry))
        .route("/api/jobs/{id}/file", get(file))
        .route("/api/cancel-all", post(cancel_all))
        .route("/api/clear", post(clear))
        .route("/api/settings", get(settings).put(put_settings))
        .route("/api/tools/update", post(update_tools))
        .fallback(|| async { error(StatusCode::NOT_FOUND, "Nicht gefunden.") })
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

// ---------------------------------------------------------------- guard

async fn guard(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let mut resp = match refuse(&app, &req) {
        Some(r) => r,
        None => next.run(req).await,
    };
    harden(resp.headers_mut());
    resp
}

/// Why a request may not pass, as the response to send instead.
fn refuse(app: &App, req: &Request) -> Option<Response> {
    let h = req.headers();
    if !app.auth.enabled() {
        if FORWARDED.iter().any(|name| h.contains_key(*name)) {
            return Some(error(
                StatusCode::FORBIDDEN,
                "Hinter einem Reverse Proxy braucht omnidl ein Passwort (--password oder OMNIDL_PASSWORD).",
            ));
        }
        let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
        if !net::is_local_host(host) {
            return Some(error(
                StatusCode::FORBIDDEN,
                "Ohne Passwort ist omnidl nur über localhost erreichbar.",
            ));
        }
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD) && !h.contains_key(CSRF_HEADER) {
        return Some(error(StatusCode::FORBIDDEN, "Anfrage abgelehnt: Kennung fehlt."));
    }
    let path = req.uri().path();
    if PUBLIC.contains(&path) || app.auth.allows(h) {
        return None;
    }
    Some(if path.starts_with("/api/") {
        error(StatusCode::UNAUTHORIZED, "Bitte anmelden.")
    } else {
        Redirect::to("/login").into_response()
    })
}

fn harden(h: &mut HeaderMap) {
    let set = |h: &mut HeaderMap, name: &'static str, value: &'static str| {
        h.insert(name, HeaderValue::from_static(value));
    };
    set(h, "content-security-policy", CSP);
    set(h, "x-content-type-options", "nosniff");
    set(h, "x-frame-options", "DENY");
    set(h, "referrer-policy", "no-referrer");
    set(h, "cross-origin-opener-policy", "same-origin");
    set(h, "cross-origin-resource-policy", "same-origin");
    if !h.contains_key(header::CACHE_CONTROL) {
        set(h, "cache-control", "no-store");
    }
}

pub fn error(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn bad_request(_: JsonRejection) -> Response {
    error(StatusCode::BAD_REQUEST, "Ungültige Anfrage.")
}

// ---------------------------------------------------------------- page

const INDEX_HTML: &str = include_str!("ui/index.html");
const LOGIN_HTML: &str = include_str!("ui/login.html");
const APP_CSS: &str = include_str!("ui/app.css");
const APP_JS: &str = include_str!("ui/app.js");
const LOGIN_JS: &str = include_str!("ui/login.js");
const MANIFEST: &str = include_str!("ui/manifest.webmanifest");
const ICON_PNG: &[u8] = include_bytes!("../../../../assets/icon-512.png");

fn asset(body: impl Into<axum::body::Body>, content_type: &'static str) -> Response {
    let mut resp = Response::new(body.into());
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    resp
}

async fn login_page(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if app.auth.allows(&headers) {
        return Redirect::to("/").into_response();
    }
    asset(LOGIN_HTML, "text/html; charset=utf-8")
}

// ---------------------------------------------------------------- login

#[derive(Deserialize)]
struct LoginReq {
    password: String,
}

async fn login(State(app): State<Arc<App>>, headers: HeaderMap, body: Result<Json<LoginReq>, JsonRejection>) -> Response {
    let Json(req) = match body {
        Ok(b) => b,
        Err(e) => return bad_request(e),
    };
    if !app.auth.enabled() {
        return StatusCode::NO_CONTENT.into_response();
    }
    match app.auth.login(&req.password).await {
        Login::Ok(token) => {
            let https = headers.get("x-forwarded-proto").is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"https"));
            let cookie = app.auth.cookie(&token, https);
            let mut resp = StatusCode::NO_CONTENT.into_response();
            if let Ok(v) = HeaderValue::from_str(&cookie) {
                resp.headers_mut().insert(header::SET_COOKIE, v);
            }
            resp
        }
        Login::Wrong => error(StatusCode::UNAUTHORIZED, "Falsches Passwort."),
        Login::Busy => error(StatusCode::TOO_MANY_REQUESTS, "Zu viele Versuche. Bitte kurz warten."),
    }
}

async fn logout(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    app.auth.logout(&headers);
    let mut resp = StatusCode::NO_CONTENT.into_response();
    if let Ok(v) = HeaderValue::from_str(&app.auth.clear_cookie()) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

// ---------------------------------------------------------------- state and events

async fn state(State(app): State<Arc<App>>) -> Json<Value> {
    let mut v = app.hub.snapshot();
    v["options"] = options();
    v["version"] = json!(update::VERSION);
    v["auth"] = json!(app.auth.enabled());
    Json(v)
}

/// Choices for the pop-up menus, labelled as in the desktop app.
fn options() -> Value {
    let formats: Vec<Value> = Format::VIDEO
        .iter()
        .chain(Format::AUDIO.iter())
        .map(|f| json!({ "id": f, "label": f.label(), "audio": f.is_audio(), "bitrate": f.has_bitrate() }))
        .collect();
    let video: Vec<Value> = VideoQuality::ALL.iter().map(|q| json!({ "id": q, "label": q.label() })).collect();
    let audio: Vec<Value> = AudioQuality::ALL.iter().map(|q| json!({ "id": q, "label": q.label() })).collect();
    let cookies: Vec<Value> = Cookies::ALL.iter().map(|c| json!({ "id": c, "label": c.label() })).collect();
    json!({ "formats": formats, "video_qualities": video, "audio_qualities": audio, "cookies": cookies })
}

async fn events(State(app): State<Arc<App>>) -> Response {
    let stream = BroadcastStream::new(app.hub.subscribe())
        .map(|msg| {
            Ok::<_, std::convert::Infallible>(match msg {
                Ok(msg) => SseEvent::default().event(msg.kind).data(&*msg.data),
                // Fell behind: the page reloads the whole state.
                Err(_) => SseEvent::default().event("reset").data("{}"),
            })
        })
        .take_until(app.shutdown.clone().cancelled_owned());
    let mut resp = Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))).into_response();
    // nginx would otherwise hold events back.
    resp.headers_mut().insert("x-accel-buffering", HeaderValue::from_static("no"));
    resp
}

// ---------------------------------------------------------------- downloads

#[derive(Deserialize)]
#[serde(untagged)]
enum Links {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct AddReq {
    urls: Links,
    mode: Option<Mode>,
    format: Option<Format>,
    video_quality: Option<VideoQuality>,
    audio_quality: Option<AudioQuality>,
    playlist: Option<bool>,
    /// Unix seconds; missing or past starts right away.
    start_at: Option<i64>,
}

async fn add(State(app): State<Arc<App>>, body: Result<Json<AddReq>, JsonRejection>) -> Response {
    let Json(req) = match body {
        Ok(b) => b,
        Err(e) => return bad_request(e),
    };
    let texts = match req.urls {
        Links::One(s) => vec![s],
        Links::Many(v) => v,
    };
    let mut seen = HashSet::new();
    let urls: Vec<String> = texts
        .iter()
        .flat_map(|t| detect::split_urls(t))
        .filter(|u| launch::is_web_link(u) && seen.insert(u.clone()))
        .collect();
    if urls.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Kein gültiger Link gefunden.");
    }
    if urls.len() > MAX_LINKS {
        return error(StatusCode::BAD_REQUEST, "Zu viele Links auf einmal.");
    }
    let now = schedule::now_unix();
    let start_at = match req.start_at {
        Some(at) if at > now + 400 * 86_400 => {
            return error(StatusCode::BAD_REQUEST, "Die Startzeit liegt zu weit in der Zukunft.");
        }
        Some(at) if at > now => Some(at),
        _ => None,
    };

    let mut cfg = app.hub.with(|m| m.cfg.clone());
    if let Some(mode) = req.mode {
        cfg.set_audio(mode == Mode::Audio);
    }
    if let Some(f) = req.format {
        cfg.set_format(f);
    }
    cfg.video_quality = req.video_quality.unwrap_or(cfg.video_quality);
    cfg.audio_quality = req.audio_quality.unwrap_or(cfg.audio_quality);
    cfg.playlist = req.playlist.unwrap_or(cfg.playlist);

    let added = urls.len();
    match app.send(Command::Add { urls, cfg, start_at }) {
        Ok(()) => Json(json!({ "added": added })).into_response(),
        Err(r) => r,
    }
}

/// Sends a command for a job the list knows.
fn job_command(app: &App, id: JobId, c: Command) -> Response {
    if app.hub.with(|m| m.jobs.get(id).is_none()) {
        return error(StatusCode::NOT_FOUND, "Unbekannter Download.");
    }
    match app.send(c) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(r) => r,
    }
}

async fn cancel(State(app): State<Arc<App>>, Path(id): Path<JobId>) -> Response {
    job_command(&app, id, Command::Cancel(id))
}

async fn start_now(State(app): State<Arc<App>>, Path(id): Path<JobId>) -> Response {
    job_command(&app, id, Command::StartNow(id))
}

/// Like the desktop app: the old entry goes, the link comes back as a new job.
async fn retry(State(app): State<Arc<App>>, Path(id): Path<JobId>) -> Response {
    let found = app.hub.with(|m| {
        let job = m.jobs.get(id)?;
        if job.parent.is_some() || !job.state.can_retry() || job.url.is_empty() {
            return Some(Err(()));
        }
        let url = job.url.clone();
        let ids: Vec<JobId> = std::iter::once(id).chain(m.jobs.children_of(id).map(|c| c.id)).collect();
        m.jobs.remove_tree(id);
        m.publish("removed", json!({ "ids": ids }));
        Some(Ok((url, m.cfg.clone())))
    });
    match found {
        None => error(StatusCode::NOT_FOUND, "Unbekannter Download."),
        Some(Err(())) => error(StatusCode::CONFLICT, "Dieser Download lässt sich nicht erneut versuchen."),
        Some(Ok((url, cfg))) => match app.send(Command::Add { urls: vec![url], cfg, start_at: None }) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(r) => r,
        },
    }
}

async fn cancel_all(State(app): State<Arc<App>>) -> Response {
    match app.send(Command::CancelAll) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(r) => r,
    }
}

/// Removes finished entries for every open page.
async fn clear(State(app): State<Arc<App>>) -> Response {
    app.hub.with(|m| {
        let before: Vec<JobId> = m.jobs.list.iter().map(|j| j.id).collect();
        m.jobs.clear_finished();
        let ids: Vec<JobId> = before.into_iter().filter(|id| m.jobs.get(*id).is_none()).collect();
        if !ids.is_empty() {
            m.publish("removed", json!({ "ids": ids }));
        }
    });
    StatusCode::NO_CONTENT.into_response()
}

async fn file(State(app): State<Arc<App>>, Path(id): Path<JobId>, headers: HeaderMap) -> Response {
    let target = app.hub.with(|m| hub::target(&m.jobs, id));
    // Folders are read outside the lock; the engine keeps reporting meanwhile.
    let offer = match target {
        Some(t) => tokio::task::spawn_blocking(move || hub::offer(t)).await.ok().flatten(),
        None => None,
    };
    match offer {
        Some(o) => files::respond(o, &headers).await,
        None => error(StatusCode::NOT_FOUND, "Zu diesem Download gibt es keine Datei."),
    }
}

// ---------------------------------------------------------------- settings

async fn settings(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.hub.with(|m| m.settings()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    download_dir: Option<String>,
    parallel: Option<usize>,
    playlist: Option<bool>,
    cookies: Option<Cookies>,
    mode: Option<Mode>,
    format: Option<Format>,
    video_quality: Option<VideoQuality>,
    audio_quality: Option<AudioQuality>,
}

async fn put_settings(State(app): State<Arc<App>>, body: Result<Json<Patch>, JsonRejection>) -> Response {
    let Json(p) = match body {
        Ok(b) => b,
        Err(e) => return bad_request(e),
    };
    let dir = match p.download_dir.map(|d| PathBuf::from(d.trim())) {
        Some(_) if app.stored_dir.is_some() => {
            return error(StatusCode::FORBIDDEN, "Der Speicherort ist beim Start mit --dir festgelegt.");
        }
        Some(d) if !d.is_absolute() => {
            return error(StatusCode::BAD_REQUEST, "Bitte einen vollständigen Pfad angeben.");
        }
        Some(d) => match tokio::fs::create_dir_all(&d).await {
            Ok(()) => Some(d),
            Err(e) => return error(StatusCode::BAD_REQUEST, &format!("Ordner nicht anlegbar: {e}")),
        },
        None => None,
    };
    if p.parallel.is_some_and(|n| !(1..=16).contains(&n)) {
        return error(StatusCode::BAD_REQUEST, "Gleichzeitige Downloads: 1 bis 16.");
    }

    let (cfg, parallel, view) = app.hub.with(|m| {
        let c = &mut m.cfg;
        let before = c.parallel;
        if let Some(d) = dir {
            c.download_dir = d;
        }
        c.parallel = p.parallel.unwrap_or(c.parallel);
        c.playlist = p.playlist.unwrap_or(c.playlist);
        c.cookies = p.cookies.unwrap_or(c.cookies);
        if let Some(mode) = p.mode {
            c.set_audio(mode == Mode::Audio);
        }
        if let Some(f) = p.format {
            c.set_format(f);
        }
        c.video_quality = p.video_quality.unwrap_or(c.video_quality);
        c.audio_quality = p.audio_quality.unwrap_or(c.audio_quality);
        let parallel = (c.parallel != before).then_some(c.parallel);
        let view = m.settings();
        m.publish("settings", view.clone());
        (m.cfg.clone(), parallel, view)
    });
    if app.save_config {
        save(&cfg, app.stored_dir.as_ref());
    }
    let _ = app.send(Command::SetConfig(cfg));
    if let Some(n) = parallel {
        let _ = app.send(Command::SetParallel(n));
    }
    Json(view).into_response()
}

/// Writes `config.toml`, keeping its own folder when `--dir` overrides it.
fn save(cfg: &Config, stored_dir: Option<&PathBuf>) {
    let mut cfg = cfg.clone();
    if let Some(dir) = stored_dir {
        cfg.download_dir = dir.clone();
    }
    cfg.save();
}

async fn update_tools(State(app): State<Arc<App>>) -> Response {
    match app.send(Command::UpdateTools) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(r) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use omnidl::job::{Event, JobState, JobUpdate};
    use tokio::sync::mpsc::UnboundedReceiver;
    use tower::ServiceExt;

    struct Test {
        app: Arc<App>,
        rx: UnboundedReceiver<Command>,
    }

    fn setup(password: Option<&str>) -> Test {
        let cfg = Config { download_dir: std::env::temp_dir().join("omnidl-tests").join("web-api"), ..Config::default() };
        let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
        let app = Arc::new(App {
            hub: Hub::new(cfg, false),
            cmd,
            auth: Auth::new(password.map(Into::into), false).with_delay(Duration::ZERO),
            stored_dir: None,
            save_config: false,
            shutdown: CancellationToken::new(),
        });
        Test { app, rx }
    }

    impl Test {
        async fn send(&self, req: axum::http::request::Builder, body: Body) -> Response {
            router(self.app.clone()).oneshot(req.body(body).unwrap()).await.unwrap()
        }

        async fn get(&self, uri: &str, cookie: Option<&str>) -> Response {
            let mut req = axum::http::Request::get(uri).header(header::HOST, "localhost:8081");
            if let Some(c) = cookie {
                req = req.header(header::COOKIE, c);
            }
            self.send(req, Body::empty()).await
        }

        async fn post(&self, uri: &str, json: Value, cookie: Option<&str>) -> Response {
            let mut req = axum::http::Request::post(uri)
                .header(header::HOST, "localhost:8081")
                .header(header::CONTENT_TYPE, "application/json")
                .header(CSRF_HEADER, "1");
            if let Some(c) = cookie {
                req = req.header(header::COOKIE, c);
            }
            self.send(req, Body::from(json.to_string())).await
        }

        /// Logs in and returns the cookie to send back.
        async fn login(&self, password: &str) -> String {
            let resp = self.post("/api/login", json!({ "password": password }), None).await;
            assert_eq!(resp.status(), StatusCode::NO_CONTENT);
            let cookie = resp.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
            cookie.split(';').next().unwrap().to_string()
        }

        fn job(&self, id: JobId, output: Option<PathBuf>) {
            let url = format!("https://example.com/{id}");
            self.app.hub.on_event(Event::Job(id, JobUpdate::New { parent: None, url: url.clone(), title: url, source: "Web" }));
            if let Some(p) = output {
                self.app.hub.on_event(Event::Job(id, JobUpdate::Output(p)));
                self.app.hub.on_event(Event::Job(id, JobUpdate::State(JobState::Done)));
            }
        }
    }

    async fn body_json(resp: Response) -> Value {
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn password_protects_everything_but_the_login() {
        let t = setup(Some("richtig"));
        assert_eq!(t.get("/api/state", None).await.status(), StatusCode::UNAUTHORIZED);
        let page = t.get("/", None).await;
        assert_eq!(page.status(), StatusCode::SEE_OTHER);
        assert_eq!(page.headers()[header::LOCATION], "/login");
        assert_eq!(t.get("/app.js", None).await.status(), StatusCode::SEE_OTHER);
        assert_eq!(t.get("/api/jobs/1/file", None).await.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(t.get("/login", None).await.status(), StatusCode::OK);
        assert_eq!(t.get("/app.css", None).await.status(), StatusCode::OK);

        let wrong = t.post("/api/login", json!({ "password": "falsch" }), None).await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        assert!(wrong.headers().get(header::SET_COOKIE).is_none());
        assert_eq!(body_json(wrong).await["error"], "Falsches Passwort.");

        let cookie = t.login("richtig").await;
        let state = t.get("/api/state", Some(&cookie)).await;
        assert_eq!(state.status(), StatusCode::OK);
        let v = body_json(state).await;
        assert_eq!(v["auth"], true);
        assert_eq!(v["options"]["formats"][0], json!({ "id": "Mp4", "label": "MP4", "audio": false, "bitrate": false }));
        assert_eq!(t.get("/", Some(&cookie)).await.status(), StatusCode::OK);
        assert_eq!(t.get("/login", Some(&cookie)).await.status(), StatusCode::SEE_OTHER, "schon angemeldet");

        assert_eq!(t.post("/api/logout", json!({}), Some(&cookie)).await.status(), StatusCode::NO_CONTENT);
        assert_eq!(t.get("/api/state", Some(&cookie)).await.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_cookie_is_locked_down() {
        let t = setup(Some("pw"));
        let resp = t.post("/api/login", json!({ "password": "pw" }), None).await;
        let cookie = resp.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.starts_with("omnidl_session="), "{cookie}");
        for flag in ["HttpOnly", "SameSite=Strict", "Path=/"] {
            assert!(cookie.contains(flag), "{flag} fehlt: {cookie}");
        }
        assert!(!cookie.contains("Secure"), "ohne HTTPS kein Secure");
    }

    #[tokio::test]
    async fn changes_need_the_csrf_header() {
        let mut t = setup(None);
        let req = axum::http::Request::post("/api/add")
            .header(header::HOST, "localhost:8081")
            .header(header::CONTENT_TYPE, "application/json");
        let body = json!({ "urls": "https://youtu.be/jNQXAC9IVRw" }).to_string();
        assert_eq!(t.send(req, Body::from(body)).await.status(), StatusCode::FORBIDDEN);
        assert!(t.rx.try_recv().is_err(), "nichts an die Engine");

        // A form post from another site can set neither the header nor JSON.
        let form = axum::http::Request::post("/api/add")
            .header(header::HOST, "localhost:8081")
            .header(header::CONTENT_TYPE, "text/plain")
            .header(CSRF_HEADER, "1");
        assert_eq!(t.send(form, Body::from("urls=x")).await.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn add_hands_links_and_choices_to_the_engine() {
        let mut t = setup(None);
        let text = "https://youtu.be/jNQXAC9IVRw\nhttps://youtu.be/jNQXAC9IVRw  file:///etc/passwd https://vimeo.com/1";
        let at = schedule::now_unix() + 1800;
        let resp = t
            .post("/api/add", json!({ "urls": text, "mode": "audio", "format": "Flac", "start_at": at }), None)
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await["added"], 2);
        match t.rx.try_recv().unwrap() {
            Command::Add { urls, cfg, start_at } => {
                assert_eq!(urls, vec!["https://youtu.be/jNQXAC9IVRw", "https://vimeo.com/1"]);
                assert_eq!(cfg.format, Format::Flac);
                assert_eq!(start_at, Some(at));
            }
            _ => panic!("Add erwartet"),
        }

        let past = t.post("/api/add", json!({ "urls": ["https://a.example/x"], "start_at": 5 }), None).await;
        assert_eq!(past.status(), StatusCode::OK);
        assert!(matches!(t.rx.try_recv().unwrap(), Command::Add { start_at: None, .. }), "Vergangenheit: sofort");

        let none = t.post("/api/add", json!({ "urls": "kein link" }), None).await;
        assert_eq!(none.status(), StatusCode::BAD_REQUEST);
        assert_eq!(t.post("/api/add", json!({ "url": "x" }), None).await.status(), StatusCode::BAD_REQUEST);
        assert!(t.rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn job_actions_reach_the_engine() {
        let mut t = setup(None);
        t.job(1, None);
        assert_eq!(t.post("/api/jobs/1/cancel", json!({}), None).await.status(), StatusCode::NO_CONTENT);
        assert!(matches!(t.rx.try_recv().unwrap(), Command::Cancel(1)));
        assert_eq!(t.post("/api/jobs/1/start-now", json!({}), None).await.status(), StatusCode::NO_CONTENT);
        assert!(matches!(t.rx.try_recv().unwrap(), Command::StartNow(1)));
        assert_eq!(t.post("/api/jobs/9/cancel", json!({}), None).await.status(), StatusCode::NOT_FOUND);
        assert_eq!(t.post("/api/jobs/1/retry", json!({}), None).await.status(), StatusCode::CONFLICT, "läuft noch");

        t.app.hub.on_event(Event::Job(1, JobUpdate::State(JobState::Failed("weg".into()))));
        let mut events = t.app.hub.subscribe();
        assert_eq!(t.post("/api/jobs/1/retry", json!({}), None).await.status(), StatusCode::NO_CONTENT);
        assert!(matches!(t.rx.try_recv().unwrap(), Command::Add { urls, .. } if urls == ["https://example.com/1"]));
        let removed = events.try_recv().unwrap();
        assert_eq!(removed.kind, "removed");
        assert!(t.app.hub.with(|m| m.jobs.list.is_empty()));

        assert_eq!(t.post("/api/cancel-all", json!({}), None).await.status(), StatusCode::NO_CONTENT);
        assert!(matches!(t.rx.try_recv().unwrap(), Command::CancelAll));
        assert_eq!(t.post("/api/tools/update", json!({}), None).await.status(), StatusCode::NO_CONTENT);
        assert!(matches!(t.rx.try_recv().unwrap(), Command::UpdateTools));
    }

    #[tokio::test]
    async fn files_only_for_known_jobs() {
        let t = setup(None);
        assert_eq!(t.get("/api/jobs/42/file", None).await.status(), StatusCode::NOT_FOUND);
        assert_eq!(t.get("/api/jobs/..%2F..%2Fetc/file", None).await.status(), StatusCode::BAD_REQUEST);
        t.job(1, None);
        assert_eq!(t.get("/api/jobs/1/file", None).await.status(), StatusCode::NOT_FOUND, "noch keine Datei");

        let dir = std::env::temp_dir().join("omnidl-tests").join("web-file");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Größe – Test.mp4");
        let data: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        std::fs::write(&path, &data).unwrap();
        t.job(2, Some(path.clone()));

        let resp = t.get("/api/jobs/2/file", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let h = resp.headers();
        assert_eq!(h[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(h[header::CONTENT_LENGTH], "200000");
        assert!(h[header::CONTENT_DISPOSITION].to_str().unwrap().contains("filename*=UTF-8''Gr%C3%B6%C3%9Fe"));
        assert_eq!(to_bytes(resp.into_body(), usize::MAX).await.unwrap(), data);

        let req = axum::http::Request::get("/api/jobs/2/file")
            .header(header::HOST, "localhost")
            .header(header::RANGE, "bytes=100000-100009");
        let part = t.send(req, Body::empty()).await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 100000-100009/200000");
        assert_eq!(to_bytes(part.into_body(), usize::MAX).await.unwrap(), data[100_000..100_010]);

        std::fs::remove_file(&path).unwrap();
        assert_eq!(t.get("/api/jobs/2/file", None).await.status(), StatusCode::NOT_FOUND, "gelöscht");
    }

    #[tokio::test]
    async fn folders_arrive_as_zip() {
        let t = setup(None);
        let dir = std::env::temp_dir().join("omnidl-tests").join("web-folder").join("TikTok 123");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.jpg"), b"eins").unwrap();
        t.job(1, Some(dir));
        let resp = t.get("/api/jobs/1/file", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "application/zip");
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        assert_eq!(zip.by_index(0).unwrap().name(), "TikTok 123/1.jpg");
    }

    #[tokio::test]
    async fn events_stream_as_sse() {
        let t = setup(None);
        let resp = t.get("/api/events", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "text/event-stream");
        assert_eq!(resp.headers()["x-accel-buffering"], "no");

        // The first event after subscribing arrives in the body.
        t.job(7, None);
        let mut body = resp.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(2), body.next()).await.unwrap().unwrap().unwrap();
        let text = String::from_utf8_lossy(&chunk);
        assert!(text.starts_with("event: job\ndata: {"), "{text}");
        assert!(text.contains("\"id\":7"), "{text}");
    }

    #[tokio::test]
    async fn without_password_only_local_direct_requests() {
        let t = setup(None);
        let get = |host: &'static str| axum::http::Request::get("/api/state").header(header::HOST, host);
        assert_eq!(t.send(get("127.0.0.1:8081"), Body::empty()).await.status(), StatusCode::OK);
        assert_eq!(t.send(get("evil.example:8081"), Body::empty()).await.status(), StatusCode::FORBIDDEN, "DNS-Rebinding");
        let proxied = get("localhost:8081").header("x-forwarded-for", "203.0.113.9");
        assert_eq!(t.send(proxied, Body::empty()).await.status(), StatusCode::FORBIDDEN, "Proxy ohne Passwort");

        // With a password the host does not matter.
        let t = setup(Some("pw"));
        let req = axum::http::Request::get("/login").header(header::HOST, "nas.fritz.box:8080");
        assert_eq!(t.send(req, Body::empty()).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn every_response_carries_security_headers() {
        let t = setup(Some("pw"));
        for resp in [t.get("/login", None).await, t.get("/api/state", None).await, t.get("/nope", None).await] {
            let h = resp.headers();
            assert!(h["content-security-policy"].to_str().unwrap().contains("frame-ancestors 'none'"));
            assert!(!h["content-security-policy"].to_str().unwrap().contains("unsafe-inline"));
            assert_eq!(h["x-content-type-options"], "nosniff");
            assert_eq!(h["x-frame-options"], "DENY");
        }
    }

    #[tokio::test]
    async fn settings_change_for_everyone() {
        let mut t = setup(None);
        let mut events = t.app.hub.subscribe();
        let resp = t
            .send(
                axum::http::Request::put("/api/settings")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(CSRF_HEADER, "1"),
                Body::from(json!({ "parallel": 2, "format": "Opus", "cookies": "Firefox" }).to_string()),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["settings"]["parallel"], 2);
        assert_eq!(v["settings"]["format"], "Opus");
        assert_eq!(v["settings"]["last_audio"], "Opus");
        assert_eq!(events.try_recv().unwrap().kind, "settings");
        assert!(matches!(t.rx.try_recv().unwrap(), Command::SetConfig(c) if c.cookies == Cookies::Firefox));
        assert!(matches!(t.rx.try_recv().unwrap(), Command::SetParallel(2)));

        let put = |body: Value| {
            let req = axum::http::Request::put("/api/settings")
                .header(header::HOST, "localhost")
                .header(header::CONTENT_TYPE, "application/json")
                .header(CSRF_HEADER, "1");
            (req, Body::from(body.to_string()))
        };
        let (req, body) = put(json!({ "parallel": 99 }));
        assert_eq!(t.send(req, body).await.status(), StatusCode::BAD_REQUEST);
        let (req, body) = put(json!({ "download_dir": "relativ/ordner" }));
        assert_eq!(t.send(req, body).await.status(), StatusCode::BAD_REQUEST);
        let (req, body) = put(json!({ "unbekannt": 1 }));
        assert_eq!(t.send(req, body).await.status(), StatusCode::BAD_REQUEST);
    }
}
