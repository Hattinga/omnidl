//! Door for links from outside the window: the browser extension and later
//! starts of omnidl itself (a second instance hands its links over and quits).
//!
//! A tiny HTTP endpoint on 127.0.0.1. It only takes links, and only from
//! callers that are not web pages: every request needs the `X-Omnidl-Client`
//! header, which a page can only send after a CORS preflight that is refused
//! for anything but extension origins. The `Host` check stops DNS rebinding.

use crate::config::Mode;
use crate::launch::{self, Launch};
use serde::Deserialize;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const PORT: u16 = 47813;
pub const CLIENT_HEADER: &str = "x-omnidl-client";
const MAX_REQUEST: usize = 64 * 1024;
const MAX_URLS: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub struct Links {
    pub urls: Vec<String>,
    pub mode: Option<Mode>,
    /// Bring the window forward (omnidl was started again) rather than stay
    /// in the background (sent from the browser).
    pub focus: bool,
}

// ---------------------------------------------------------------- requests

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug)]
pub enum Parse {
    Incomplete,
    Done(Request),
    Invalid,
}

pub fn parse(buf: &[u8]) -> Parse {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if buf.len() >= MAX_REQUEST { Parse::Invalid } else { Parse::Incomplete };
    };
    let Ok(head) = std::str::from_utf8(&buf[..end]) else { return Parse::Invalid };
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (Some(method), Some(path), Some(version)) = (first.next(), first.next(), first.next()) else {
        return Parse::Invalid;
    };
    if !version.starts_with("HTTP/1.") {
        return Parse::Invalid;
    }
    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { return Parse::Invalid };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let mut req = Request { method: method.to_string(), path: path.to_string(), headers, body: Vec::new() };
    if req.header("transfer-encoding").is_some() {
        return Parse::Invalid;
    }
    let len = match req.header("content-length").map(str::parse::<usize>) {
        None => 0,
        Some(Ok(n)) if n <= MAX_REQUEST => n,
        Some(_) => return Parse::Invalid,
    };
    let start = end + 4;
    if buf.len() < start + len {
        return Parse::Incomplete;
    }
    req.body = buf[start..start + len].to_vec();
    Parse::Done(req)
}

pub struct Response {
    pub status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

impl Response {
    fn new(status: u16, body: impl Into<String>) -> Self {
        Self { status, headers: Vec::new(), body: body.into() }
    }

    fn json(status: u16, value: serde_json::Value) -> Self {
        let mut r = Self::new(status, value.to_string());
        r.headers.push(("Content-Type", "application/json".into()));
        r
    }

    #[cfg(test)]
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            202 => "Accepted",
            204 => "No Content",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            _ => "Error",
        };
        let mut s = format!("HTTP/1.1 {} {reason}\r\n", self.status);
        for (n, v) in &self.headers {
            s.push_str(&format!("{n}: {v}\r\n"));
        }
        s.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", self.body.len()));
        s.push_str(&self.body);
        s.into_bytes()
    }
}

/// Browser extensions of Chrome, Edge, Brave, Firefox and Safari.
fn is_extension_origin(origin: &str) -> bool {
    ["chrome-extension://", "moz-extension://", "safari-web-extension://", "extension://"]
        .iter()
        .any(|p| origin.starts_with(p))
}

#[derive(Deserialize)]
struct AddBody {
    #[serde(default)]
    urls: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    focus: bool,
}

/// Answers one request; the links, if any, are what the caller should add.
pub fn route(req: &Request, port: u16) -> (Response, Option<Links>) {
    let host_ok = req
        .header("host")
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    if !host_ok {
        return (Response::new(403, "host"), None);
    }
    let origin = req.header("origin");
    if origin.is_some_and(|o| !is_extension_origin(o)) {
        return (Response::new(403, "origin"), None);
    }
    let cors = |mut r: Response| {
        if let Some(o) = origin {
            r.headers.push(("Access-Control-Allow-Origin", o.to_string()));
            r.headers.push(("Vary", "Origin".into()));
        }
        r
    };

    if req.method == "OPTIONS" {
        if origin.is_none() {
            return (Response::new(403, "origin"), None);
        }
        let mut r = Response::new(204, "");
        r.headers.push(("Access-Control-Allow-Methods", "GET, POST".into()));
        r.headers.push(("Access-Control-Allow-Headers", format!("Content-Type, {CLIENT_HEADER}")));
        r.headers.push(("Access-Control-Allow-Private-Network", "true".into()));
        r.headers.push(("Access-Control-Max-Age", "600".into()));
        return (cors(r), None);
    }
    if req.header(CLIENT_HEADER).is_none_or(str::is_empty) {
        return (Response::new(403, "client"), None);
    }

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/ping") => (
            cors(Response::json(200, serde_json::json!({ "app": "omnidl", "version": env!("CARGO_PKG_VERSION") }))),
            None,
        ),
        ("POST", "/v1/add") => {
            let Ok(body) = serde_json::from_slice::<AddBody>(&req.body) else {
                return (cors(Response::json(400, serde_json::json!({ "error": "JSON erwartet" }))), None);
            };
            let urls: Vec<String> = body
                .urls
                .into_iter()
                .chain(body.url)
                .map(|u| u.trim().to_string())
                .filter(|u| launch::is_web_link(u))
                .take(MAX_URLS)
                .collect();
            if urls.is_empty() && !body.focus {
                return (cors(Response::json(400, serde_json::json!({ "error": "keine gültigen Links" }))), None);
            }
            let links = Links { mode: body.mode.as_deref().and_then(launch::mode_from), focus: body.focus, urls };
            let resp = Response::json(202, serde_json::json!({ "ok": true, "added": links.urls.len() }));
            (cors(resp), Some(links))
        }
        (_, "/v1/ping" | "/v1/add") => (cors(Response::new(405, "")), None),
        _ => (cors(Response::new(404, "")), None),
    }
}

// ---------------------------------------------------------------- server

pub async fn serve(listener: tokio::net::TcpListener, port: u16, on_links: Arc<dyn Fn(Links) + Send + Sync>) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else { continue };
        if !peer.ip().is_loopback() {
            continue;
        }
        let on_links = on_links.clone();
        tokio::spawn(async move { connection(stream, port, on_links).await });
    }
}

async fn connection(mut stream: tokio::net::TcpStream, port: u16, on_links: Arc<dyn Fn(Links) + Send + Sync>) {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let reply = loop {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk)).await;
        let n = match read {
            Ok(Ok(n)) if n > 0 => n,
            _ => return,
        };
        buf.extend_from_slice(&chunk[..n]);
        match parse(&buf) {
            Parse::Incomplete if buf.len() < MAX_REQUEST => continue,
            Parse::Incomplete => break (Response::new(413, ""), None),
            Parse::Invalid => break (Response::new(400, ""), None),
            Parse::Done(req) => break route(&req, port),
        }
    };
    let (resp, links) = reply;
    let _ = stream.write_all(&resp.to_bytes()).await;
    let _ = stream.shutdown().await;
    if let Some(l) = links {
        on_links(l);
    }
}

// ---------------------------------------------------------------- client

/// Sends links to the omnidl listening on `port`; returns the HTTP status.
pub fn send(port: u16, links: &Links) -> std::io::Result<u16> {
    let body = serde_json::json!({ "urls": links.urls, "mode": links.mode, "focus": links.focus }).to_string();
    let head = format!(
        "POST /v1/add HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{CLIENT_HEADER}: launcher\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let (status, _) = exchange(port, format!("{head}{body}").as_bytes())?;
    Ok(status)
}

/// Whether an omnidl answers on `port`.
pub fn ping(port: u16) -> bool {
    let req = format!("GET /v1/ping HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{CLIENT_HEADER}: launcher\r\nConnection: close\r\n\r\n");
    matches!(exchange(port, req.as_bytes()), Ok((200, body)) if body.contains("\"app\":\"omnidl\""))
}

fn exchange(port: u16, request: &[u8]) -> std::io::Result<(u16, String)> {
    let timeout = Duration::from_secs(3);
    let mut s = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.write_all(request)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "keine HTTP-Antwort"))?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Ok((status, body))
}

// ---------------------------------------------------------------- single instance

pub enum Claim {
    /// This instance owns the port and serves it.
    Owner(std::net::TcpListener),
    /// A running omnidl took the links; this instance can quit.
    Forwarded,
    /// Runs without the bridge (port taken by something else).
    Unavailable(String),
}

pub fn claim(launch: &Launch) -> Claim {
    claim_on(PORT, launch)
}

fn claim_on(port: u16, launch: &Launch) -> Claim {
    // After an update the old instance is still shutting down.
    let deadline = Instant::now() + Duration::from_secs(if launch.restarted { 10 } else { 0 });
    loop {
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => return Claim::Owner(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                if !launch.restarted && ping(port) {
                    let links = Links { urls: launch.urls.clone(), mode: launch.mode, focus: true };
                    if matches!(send(port, &links), Ok(202)) {
                        return Claim::Forwarded;
                    }
                }
                return Claim::Unavailable(format!("Port {port} ist von einem anderen Programm belegt"));
            }
            Err(e) => return Claim::Unavailable(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const P: u16 = 47813;

    fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
        let mut raw = format!("{method} {path} HTTP/1.1\r\n");
        for (n, v) in headers {
            raw.push_str(&format!("{n}: {v}\r\n"));
        }
        raw.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        match parse(raw.as_bytes()) {
            Parse::Done(r) => r,
            other => panic!("nicht geparst: {other:?}"),
        }
    }

    fn from_extension(body: &str) -> Request {
        request(
            "POST",
            "/v1/add",
            &[("Host", "127.0.0.1:47813"), ("Origin", "chrome-extension://abc"), ("X-Omnidl-Client", "extension")],
            body,
        )
    }

    #[test]
    fn parses_in_pieces() {
        let raw = b"POST /v1/add HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello";
        assert!(matches!(parse(&raw[..20]), Parse::Incomplete));
        assert!(matches!(parse(&raw[..raw.len() - 2]), Parse::Incomplete), "Body fehlt noch");
        let Parse::Done(r) = parse(raw) else { panic!() };
        assert_eq!((r.method.as_str(), r.path.as_str(), r.body.as_slice()), ("POST", "/v1/add", &b"hello"[..]));
        assert_eq!(r.header("host"), Some("x"), "Namen ohne Groß/klein");
    }

    #[test]
    fn rejects_malformed_requests() {
        assert!(matches!(parse(b"GARBAGE\r\n\r\n"), Parse::Invalid));
        assert!(matches!(parse(b"GET / SPDY/3\r\n\r\n"), Parse::Invalid));
        assert!(matches!(parse(b"GET / HTTP/1.1\r\nno colon\r\n\r\n"), Parse::Invalid));
        assert!(matches!(parse(b"POST / HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n"), Parse::Invalid));
        assert!(matches!(parse(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"), Parse::Invalid));
        assert!(matches!(parse(&vec![b'a'; MAX_REQUEST]), Parse::Invalid));
    }

    #[test]
    fn extension_can_add_links() {
        let req = from_extension(r#"{"urls":["https://youtu.be/a"," https://x.com/u/status/1 "],"mode":"audio"}"#);
        let (resp, links) = route(&req, P);
        assert_eq!(resp.status, 202);
        assert_eq!(resp.header("Access-Control-Allow-Origin"), Some("chrome-extension://abc"));
        let links = links.unwrap();
        assert_eq!(links.urls, vec!["https://youtu.be/a", "https://x.com/u/status/1"]);
        assert_eq!(links.mode, Some(Mode::Audio));
        assert!(!links.focus);
    }

    #[test]
    fn single_url_field_and_junk_filtered() {
        let (resp, links) = route(&from_extension(r#"{"url":"https://vimeo.com/1","urls":["file:///C:/x","javascript:1"]}"#), P);
        assert_eq!(resp.status, 202);
        assert_eq!(links.unwrap().urls, vec!["https://vimeo.com/1"]);

        let (resp, links) = route(&from_extension(r#"{"urls":["ftp://x"]}"#), P);
        assert_eq!((resp.status, links), (400, None));
        let (resp, _) = route(&from_extension("kein json"), P);
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn web_pages_are_turned_away() {
        let page = [("Host", "127.0.0.1:47813"), ("Origin", "https://evil.example"), ("X-Omnidl-Client", "x")];
        let (resp, links) = route(&request("POST", "/v1/add", &page, r#"{"urls":["https://a.com"]}"#), P);
        assert_eq!((resp.status, links), (403, None));

        // Preflight from a page: refused, so the browser never sends the real request.
        let (resp, _) = route(&request("OPTIONS", "/v1/add", &page[..2], ""), P);
        assert_eq!(resp.status, 403);
        assert_eq!(resp.header("Access-Control-Allow-Origin"), None);

        // Simple request without the custom header (form post).
        let form = [("Host", "127.0.0.1:47813")];
        let (resp, _) = route(&request("POST", "/v1/add", &form, r#"{"urls":["https://a.com"]}"#), P);
        assert_eq!(resp.status, 403);

        // DNS rebinding: the page's own host name arrives in Host.
        let rebound = [("Host", "evil.example:47813"), ("X-Omnidl-Client", "x")];
        let (resp, _) = route(&request("GET", "/v1/ping", &rebound, ""), P);
        assert_eq!(resp.status, 403);
    }

    #[test]
    fn preflight_from_extension_is_allowed() {
        let h = [("Host", "localhost:47813"), ("Origin", "moz-extension://1234")];
        let (resp, _) = route(&request("OPTIONS", "/v1/add", &h, ""), P);
        assert_eq!(resp.status, 204);
        assert!(resp.header("Access-Control-Allow-Headers").unwrap().contains(CLIENT_HEADER));
        assert_eq!(resp.header("Access-Control-Allow-Private-Network"), Some("true"));
    }

    #[test]
    fn ping_and_unknown_paths() {
        let h = [("Host", "127.0.0.1:47813"), ("X-Omnidl-Client", "launcher")];
        let (resp, _) = route(&request("GET", "/v1/ping", &h, ""), P);
        assert_eq!(resp.status, 200);
        assert!(String::from_utf8(resp.to_bytes()).unwrap().contains("\"app\":\"omnidl\""));
        assert_eq!(route(&request("GET", "/v1/add", &h, ""), P).0.status, 405);
        assert_eq!(route(&request("GET", "/", &h, ""), P).0.status, 404);
        // Focus without links: a second start of the app.
        let (resp, links) = route(&request("POST", "/v1/add", &h, r#"{"focus":true}"#), P);
        assert_eq!(resp.status, 202);
        assert_eq!(links, Some(Links { urls: vec![], mode: None, focus: true }));
    }

    /// Echter Durchlauf über TCP: zweiter Start reicht Links an den ersten weiter.
    #[test]
    fn second_instance_hands_links_to_the_first() {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = std_listener.local_addr().unwrap().port();
        std_listener.set_nonblocking(true).unwrap();
        let got: Arc<Mutex<Vec<Links>>> = Arc::default();
        {
            let got = got.clone();
            let _guard = rt.enter();
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            rt.spawn(serve(listener, port, Arc::new(move |l| got.lock().unwrap().push(l))));
        }

        assert!(ping(port), "erste Instanz antwortet");
        let launch = Launch { urls: vec!["https://youtu.be/abc".into()], mode: Some(Mode::Video), ..Default::default() };
        assert!(matches!(claim_on(port, &launch), Claim::Forwarded));

        let deadline = Instant::now() + Duration::from_secs(3);
        while got.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], Links { urls: launch.urls.clone(), mode: Some(Mode::Video), focus: true });
    }

    #[test]
    fn foreign_program_on_the_port_is_not_mistaken_for_omnidl() {
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = other.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in other.incoming().flatten() {
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        });
        assert!(!ping(port));
        let launch = Launch { urls: vec!["https://a.com".into()], ..Default::default() };
        assert!(matches!(claim_on(port, &launch), Claim::Unavailable(_)));
    }

    #[test]
    fn port_is_free_again_right_after_use() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        });
        let _ = exchange(port, b"GET / HTTP/1.1\r\n\r\n");
        t.join().unwrap();
        // Nach einem Update bindet die neue Instanz denselben Port.
        assert!(matches!(claim_on(port, &Launch { restarted: true, ..Default::default() }), Claim::Owner(_)));
    }
}
