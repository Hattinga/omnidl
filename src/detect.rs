use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpotifyKind {
    Track,
    Album,
    Playlist,
    Artist,
}

impl SpotifyKind {
    pub fn path(self) -> &'static str {
        match self {
            SpotifyKind::Track => "track",
            SpotifyKind::Album => "album",
            SpotifyKind::Playlist => "playlist",
            SpotifyKind::Artist => "artist",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    YouTube { playlist: bool },
    TikTok,
    TikTokPhoto,
    Spotify(SpotifyKind, String),
    SpotifyShort,
    Instagram,
    Twitter,
    SoundCloud { set: bool },
    Generic,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::YouTube { playlist: false } => "YouTube",
            Source::YouTube { playlist: true } => "YouTube-Playlist",
            Source::TikTok => "TikTok",
            Source::TikTokPhoto => "TikTok-Fotos",
            Source::Spotify(SpotifyKind::Track, _) => "Spotify-Track",
            Source::Spotify(SpotifyKind::Album, _) => "Spotify-Album",
            Source::Spotify(SpotifyKind::Playlist, _) => "Spotify-Playlist",
            Source::Spotify(SpotifyKind::Artist, _) => "Spotify-Künstler",
            Source::SpotifyShort => "Spotify",
            Source::Instagram => "Instagram",
            Source::Twitter => "X / Twitter",
            Source::SoundCloud { set: false } => "SoundCloud",
            Source::SoundCloud { set: true } => "SoundCloud-Set",
            Source::Generic => "Web",
        }
    }

    /// Whether this URL most likely points to a list that should be expanded into
    /// one job per entry (parallel downloads).
    pub fn is_list(&self) -> bool {
        matches!(
            self,
            Source::YouTube { playlist: true } | Source::SoundCloud { set: true }
        )
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    host.strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(&host)
        .to_string()
}

fn spotify_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:open\.spotify\.com/(?:intl-[a-z-]+/)?(?:embed/)?|spotify:)(track|album|playlist|artist)[/:]([A-Za-z0-9]{22})",
        )
        .unwrap()
    })
}

pub fn detect(url: &str) -> Source {
    let url = url.trim();
    if let Some(c) = spotify_re().captures(url) {
        let kind = match c[1].to_ascii_lowercase().as_str() {
            "track" => SpotifyKind::Track,
            "album" => SpotifyKind::Album,
            "playlist" => SpotifyKind::Playlist,
            _ => SpotifyKind::Artist,
        };
        return Source::Spotify(kind, c[2].to_string());
    }
    let host = host_of(url);
    let path = url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(url)
        .split_once('/')
        .map(|(_, p)| p)
        .unwrap_or("");
    match host.as_str() {
        "spotify.link" => Source::SpotifyShort,
        h if h == "youtu.be" || h.ends_with("youtube.com") => {
            let is_list = path.starts_with("playlist")
                || path.contains("list=")
                || path.starts_with('@')
                || path.starts_with("channel/")
                || path.starts_with("c/")
                || path.starts_with("user/");
            Source::YouTube { playlist: is_list }
        }
        h if h.ends_with("tiktok.com") => {
            if path.contains("/photo/") { Source::TikTokPhoto } else { Source::TikTok }
        }
        h if h.ends_with("instagram.com") => Source::Instagram,
        "x.com" | "twitter.com" | "mobile.twitter.com" => Source::Twitter,
        h if h.ends_with("soundcloud.com") => Source::SoundCloud { set: path.contains("/sets/") },
        _ => Source::Generic,
    }
}

/// Splits pasted text into URLs (one per line or whitespace separated).
pub fn split_urls(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|s| s.trim_matches(|c: char| c == '<' || c == '>' || c == '"' || c == '\''))
        .filter(|s| s.contains("://") || s.starts_with("spotify:") || s.contains('.'))
        .map(|s| {
            if s.contains("://") || s.starts_with("spotify:") {
                s.to_string()
            } else {
                format!("https://{s}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube() {
        assert_eq!(detect("https://www.youtube.com/watch?v=abc"), Source::YouTube { playlist: false });
        assert_eq!(detect("https://youtu.be/abc"), Source::YouTube { playlist: false });
        assert_eq!(detect("https://youtube.com/shorts/abc"), Source::YouTube { playlist: false });
        assert_eq!(detect("https://www.youtube.com/watch?v=a&list=PL1"), Source::YouTube { playlist: true });
        assert_eq!(detect("https://www.youtube.com/playlist?list=PL1"), Source::YouTube { playlist: true });
        assert_eq!(detect("https://m.youtube.com/@channel"), Source::YouTube { playlist: true });
        assert_eq!(detect("https://music.youtube.com/watch?v=abc"), Source::YouTube { playlist: false });
    }

    #[test]
    fn spotify() {
        assert_eq!(
            detect("https://open.spotify.com/intl-de/track/2FZcjBYK4dTt48q94pJbJD?si=x"),
            Source::Spotify(SpotifyKind::Track, "2FZcjBYK4dTt48q94pJbJD".into())
        );
        assert_eq!(
            detect("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"),
            Source::Spotify(SpotifyKind::Playlist, "37i9dQZF1DXcBWIGoYBM5M".into())
        );
        assert_eq!(
            detect("https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy"),
            Source::Spotify(SpotifyKind::Album, "4aawyAB9vmqN3uQ7FjRGTy".into())
        );
        assert_eq!(detect("https://spotify.link/abcdef"), Source::SpotifyShort);
    }

    #[test]
    fn others() {
        assert_eq!(detect("https://www.tiktok.com/@u/video/123"), Source::TikTok);
        assert_eq!(detect("https://www.tiktok.com/@u/photo/123"), Source::TikTokPhoto);
        assert_eq!(detect("https://vm.tiktok.com/ZMabc/"), Source::TikTok);
        assert_eq!(detect("https://x.com/u/status/1"), Source::Twitter);
        assert_eq!(detect("https://soundcloud.com/a/sets/b"), Source::SoundCloud { set: true });
        assert_eq!(detect("https://vimeo.com/1"), Source::Generic);
    }

    #[test]
    fn split() {
        let v = split_urls("https://a.com/x\n youtu.be/abc  <https://b.com>\nfoo");
        assert_eq!(v, vec!["https://a.com/x", "https://youtu.be/abc", "https://b.com"]);
    }
}
