//! Password and sessions. Without a password (only possible on loopback) every
//! request from this machine is let in; with one, a login sets a random
//! session cookie that the API checks on every request.

use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub const COOKIE: &str = "omnidl_session";
/// How long a login lasts.
const SESSION_DAYS: u64 = 30;
const MAX_SESSIONS: usize = 256;
/// Failed logins waiting in line; more are turned away right away.
const MAX_WAITING: usize = 16;

/// Where the password came from, for the startup message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// `--password` or `OMNIDL_PASSWORD`.
    Given,
    /// Generated earlier and read from `web.toml`.
    Stored,
    /// Generated just now and written to `web.toml`.
    Generated,
}

/// Contents of `<base>/web.toml`.
#[derive(Default, Serialize, Deserialize)]
struct WebFile {
    password: Option<String>,
}

/// Picks the password: given > stored > new. `None` means no login, which is
/// only allowed when nobody else can reach the server.
pub fn resolve(given: Option<String>, loopback: bool, file: &Path) -> std::io::Result<Option<(String, Origin)>> {
    if let Some(p) = given.filter(|p| !p.is_empty()) {
        return Ok(Some((p, Origin::Given)));
    }
    if loopback {
        return Ok(None);
    }
    if let Some(p) = load(file) {
        return Ok(Some((p, Origin::Stored)));
    }
    let p = generate();
    store(file, &p)?;
    Ok(Some((p, Origin::Generated)))
}

fn load(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    toml::from_str::<WebFile>(&text).ok()?.password.filter(|p| !p.is_empty())
}

fn store(file: &Path, password: &str) -> std::io::Result<()> {
    let body = toml::to_string(&WebFile { password: Some(password.into()) }).map_err(std::io::Error::other)?;
    let text = format!(
        "# omnidl Web-Interface: Passwort für den Zugriff aus dem Netzwerk.\n\
         # Löschen erzeugt beim nächsten Start ein neues.\n{body}"
    );
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    std::io::Write::write_all(&mut opts.open(file)?, text.as_bytes())
}

/// Four groups of five from an alphabet without look-alikes (about 99 bits),
/// easy to type on a phone: `k7mq2-xh4pt-9vwc3-rj8nd`.
pub fn generate() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut out = String::with_capacity(23);
    let mut n = 0;
    while n < 20 {
        // Rejection sampling keeps every character equally likely.
        let b = random::<1>()[0];
        if (b as usize) < 256 - 256 % ALPHABET.len() {
            if n > 0 && n % 5 == 0 {
                out.push('-');
            }
            out.push(ALPHABET[b as usize % ALPHABET.len()] as char);
            n += 1;
        }
    }
    out
}

fn random<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("Zufallsquelle des Systems nicht verfügbar");
    buf
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

/// Compares in constant time; hashing first hides the length too.
pub fn same_secret(a: &str, b: &str) -> bool {
    let (x, y) = (digest(a), digest(b));
    x.iter().zip(y.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

pub struct Auth {
    password: Option<String>,
    /// Hash of the token → expiry. Only hashes are kept, so a lookup reveals nothing.
    sessions: Mutex<HashMap<[u8; 32], Instant>>,
    /// Failed attempts take turns, so guessing is limited to one per delay.
    gate: tokio::sync::Mutex<()>,
    waiting: AtomicUsize,
    fail_delay: Duration,
    /// Send the cookie over HTTPS only.
    secure: bool,
}

pub enum Login {
    Ok(String),
    Wrong,
    Busy,
}

impl Auth {
    pub fn new(password: Option<String>, secure: bool) -> Self {
        Self {
            password,
            sessions: Mutex::new(HashMap::new()),
            gate: tokio::sync::Mutex::new(()),
            waiting: AtomicUsize::new(0),
            fail_delay: Duration::from_millis(1000),
            secure,
        }
    }

    #[cfg(test)]
    pub fn with_delay(mut self, d: Duration) -> Self {
        self.fail_delay = d;
        self
    }

    pub fn enabled(&self) -> bool {
        self.password.is_some()
    }

    /// Checks the password; on success returns a new session token.
    pub async fn login(&self, given: &str) -> Login {
        let Some(password) = &self.password else { return Login::Ok(self.new_session()) };
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= MAX_WAITING {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return Login::Busy;
        }
        let result = {
            let _turn = self.gate.lock().await;
            if same_secret(given, password) {
                Login::Ok(self.new_session())
            } else {
                tokio::time::sleep(self.fail_delay).await;
                Login::Wrong
            }
        };
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn new_session(&self) -> String {
        let token = hex(&random::<32>());
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, expiry| *expiry > now);
        if sessions.len() >= MAX_SESSIONS {
            // Drop the oldest login rather than refusing a new one.
            if let Some(oldest) = sessions.iter().min_by_key(|(_, e)| **e).map(|(k, _)| *k) {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(digest(&token), now + Duration::from_secs(SESSION_DAYS * 86_400));
        token
    }

    /// No password, or a live session cookie.
    pub fn allows(&self, headers: &HeaderMap) -> bool {
        if !self.enabled() {
            return true;
        }
        let Some(token) = session_cookie(headers) else { return false };
        let sessions = self.sessions.lock().unwrap();
        sessions.get(&digest(token)).is_some_and(|expiry| *expiry > Instant::now())
    }

    pub fn logout(&self, headers: &HeaderMap) {
        if let Some(token) = session_cookie(headers) {
            self.sessions.lock().unwrap().remove(&digest(token));
        }
    }

    /// `Set-Cookie` for a new session. `https`: the request came through an HTTPS proxy.
    pub fn cookie(&self, token: &str, https: bool) -> String {
        let secure = if self.secure || https { "; Secure" } else { "" };
        let max_age = SESSION_DAYS * 86_400;
        format!("{COOKIE}={token}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Strict{secure}")
    }

    pub fn clear_cookie(&self) -> String {
        format!("{COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict")
    }
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value)
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("omnidl-tests").join("web-auth");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn generated_passwords_are_long_and_readable() {
        let p = generate();
        assert_eq!(p.len(), 23, "{p}");
        let groups: Vec<&str> = p.split('-').collect();
        assert_eq!(groups.len(), 4);
        assert!(groups.iter().all(|g| g.len() == 5));
        assert!(!p.contains(['0', 'o', '1', 'l', 'i']), "keine Verwechsler: {p}");
        assert_ne!(generate(), generate());
    }

    #[test]
    fn password_is_generated_once_and_then_kept() {
        let file = temp("web.toml");
        let (first, origin) = resolve(None, false, &file).unwrap().unwrap();
        assert_eq!(origin, Origin::Generated);
        let (again, origin) = resolve(None, false, &file).unwrap().unwrap();
        assert_eq!((again.as_str(), origin), (first.as_str(), Origin::Stored));
        assert!(std::fs::read_to_string(&file).unwrap().contains(&first));
    }

    #[test]
    fn given_password_wins_and_loopback_needs_none() {
        let file = temp("given.toml");
        let given = resolve(Some("geheim".into()), false, &file).unwrap();
        assert_eq!(given, Some(("geheim".into(), Origin::Given)));
        assert_eq!(resolve(Some("geheim".into()), true, &file).unwrap().unwrap().1, Origin::Given);
        assert_eq!(resolve(None, true, &file).unwrap(), None);
        assert_eq!(resolve(Some(String::new()), true, &file).unwrap(), None, "leer heißt keins");
        assert!(!file.exists(), "nichts erzeugt, wenn nichts gebraucht wird");
    }

    #[test]
    fn secrets_compare_exactly() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "abcd"));
        assert!(!same_secret("", "x"));
    }

    fn with_cookie(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::COOKIE, HeaderValue::from_str(value).unwrap());
        h
    }

    #[tokio::test]
    async fn sessions_after_login_only() {
        let auth = Auth::new(Some("pw".into()), false).with_delay(Duration::ZERO);
        assert!(!auth.allows(&HeaderMap::new()));
        assert!(matches!(auth.login("falsch").await, Login::Wrong));
        let Login::Ok(token) = auth.login("pw").await else { panic!("Anmeldung klappt") };
        assert_eq!(token.len(), 64);
        let headers = with_cookie(&format!("theme=dark; {COOKIE}={token}"));
        assert!(auth.allows(&headers));
        assert!(!auth.allows(&with_cookie(&format!("{COOKIE}={}", "0".repeat(64)))));
        assert!(!auth.allows(&with_cookie(&format!("{COOKIE}="))));
        auth.logout(&headers);
        assert!(!auth.allows(&headers), "abgemeldet");
    }

    #[test]
    fn cookie_flags() {
        let auth = Auth::new(Some("pw".into()), false);
        let c = auth.cookie("t", false);
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict") && c.contains("Path=/"), "{c}");
        assert!(!c.contains("Secure"));
        assert!(auth.cookie("t", true).ends_with("; Secure"), "hinter HTTPS-Proxy");
        assert!(Auth::new(None, true).cookie("t", false).ends_with("; Secure"), "--secure-cookie");
        assert!(Auth::new(None, false).allows(&HeaderMap::new()), "ohne Passwort offen");
    }
}
