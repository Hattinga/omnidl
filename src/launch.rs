//! What omnidl was started with: links passed on the command line, an
//! `omnidl://` link from the browser, the restart after an update, or the
//! clean-up call from the uninstaller.

use crate::config::Mode;

pub const SCHEME: &str = "omnidl";
/// Set by the updater so the new instance waits for the old one to exit.
pub const RESTARTED: &str = "--restarted";
/// Run by the uninstaller: remove what the app created, then quit.
pub const UNINSTALL: &str = "--uninstall";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub urls: Vec<String>,
    pub mode: Option<Mode>,
    pub restarted: bool,
    pub uninstall: bool,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Launch {
    let mut l = Launch::default();
    for arg in args {
        if arg == RESTARTED {
            l.restarted = true;
        } else if arg == UNINSTALL {
            l.uninstall = true;
        } else if let Some((urls, mode)) = parse_link(&arg) {
            l.urls.extend(urls);
            l.mode = mode.or(l.mode);
        } else if is_web_link(&arg) {
            l.urls.push(arg);
        }
    }
    l
}

/// `omnidl://add?url=<encoded>&url=…&mode=audio`
pub fn parse_link(s: &str) -> Option<(Vec<String>, Option<Mode>)> {
    let rest = s.trim().strip_prefix(SCHEME)?.strip_prefix(':')?;
    let rest = rest.trim_start_matches('/');
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    if !action.trim_end_matches('/').eq_ignore_ascii_case("add") {
        return None;
    }
    let mut urls = Vec::new();
    let mut mode = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = urlencoding::decode(value).map(|v| v.into_owned()).unwrap_or_default();
        match key {
            "url" if is_web_link(&value) => urls.push(value),
            "mode" => mode = mode_from(&value),
            _ => {}
        }
    }
    Some((urls, mode))
}

pub fn mode_from(s: &str) -> Option<Mode> {
    match s.to_ascii_lowercase().as_str() {
        "audio" => Some(Mode::Audio),
        "video" => Some(Mode::Video),
        _ => None,
    }
}

/// Only web pages and Spotify URIs; nothing that could point at local files.
pub fn is_web_link(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    s.len() <= 4096
        && !s.chars().any(|c| c.is_control() || c.is_whitespace())
        && (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("spotify:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same encoding as the browser extension (`omnidlLink` in omnidl.js).
    fn build_link(urls: &[String], mode: Option<Mode>) -> String {
        let parts: Vec<String> = urls.iter().map(|u| format!("url={}", urlencoding::encode(u))).collect();
        let mode = match mode {
            Some(Mode::Audio) => "&mode=audio",
            Some(Mode::Video) => "&mode=video",
            None => "",
        };
        format!("{SCHEME}://add?{}{mode}", parts.join("&"))
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn links_from_the_browser() {
        let l = parse(args(&["omnidl://add?url=https%3A%2F%2Fyoutu.be%2Fabc%3Ft%3D1&mode=audio"]));
        assert_eq!(l.urls, vec!["https://youtu.be/abc?t=1"]);
        assert_eq!(l.mode, Some(Mode::Audio));
        assert!(!l.restarted);

        // Browsers sometimes add or drop the slashes.
        let (urls, mode) = parse_link("omnidl:add?url=https://a.com/x&url=https://b.com/").unwrap();
        assert_eq!(urls, vec!["https://a.com/x", "https://b.com/"]);
        assert_eq!(mode, None);
        assert!(parse_link("omnidl://add/?url=https://a.com").is_some());
    }

    #[test]
    fn roundtrip_through_build_link() {
        let urls = args(&["https://www.youtube.com/watch?v=a&list=PL1", "spotify:track:2FZcjBYK4dTt48q94pJbJD"]);
        let (back, mode) = parse_link(&build_link(&urls, Some(Mode::Video))).unwrap();
        assert_eq!(back, urls);
        assert_eq!(mode, Some(Mode::Video));
    }

    #[test]
    fn refuses_anything_but_web_links() {
        let (urls, _) = parse_link("omnidl://add?url=file%3A%2F%2F%2FC%3A%2FWindows&url=javascript%3Aalert(1)").unwrap();
        assert!(urls.is_empty());
        assert!(parse_link("omnidl://delete?url=https://a.com").is_none());
        assert!(parse_link("https://a.com").is_none());
        assert!(!is_web_link("https://a.com/\nx"));
        assert!(!is_web_link(&format!("https://a.com/{}", "x".repeat(5000))));
    }

    #[test]
    fn plain_arguments_and_flags() {
        let l = parse(args(&["--restarted", "https://vimeo.com/1", "--unknown", "C:\\datei.txt"]));
        assert!(l.restarted);
        assert!(!l.uninstall);
        assert!(parse(args(&["--uninstall"])).uninstall);
        assert_eq!(l.urls, vec!["https://vimeo.com/1"]);
        assert_eq!(parse(args(&[])), Launch::default());
    }
}
