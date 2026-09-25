//! Quick title lookup through oEmbed, so a job shows its real name while it
//! waits (scheduled, queued, starting) instead of the bare link. yt-dlp only
//! reports the title once the download begins.

use crate::detect::{self, Source};
use crate::util;
use std::time::Duration;

/// oEmbed endpoint for a single item on the big sites; `None` for lists
/// (their name comes from the track list) and for sites without one.
pub fn endpoint(url: &str) -> Option<String> {
    let base = match detect::detect(url) {
        Source::YouTube { playlist: false } => "https://www.youtube.com/oembed?format=json&url=",
        Source::TikTok => "https://www.tiktok.com/oembed?url=",
        Source::SoundCloud { set: false } => "https://soundcloud.com/oembed?format=json&url=",
        Source::Spotify(..) => "https://open.spotify.com/oembed?url=",
        Source::Generic if is_vimeo(url) => "https://vimeo.com/api/oembed.json?url=",
        _ => return None,
    };
    Some(format!("{base}{}", urlencoding::encode(url)))
}

fn is_vimeo(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r).to_ascii_lowercase();
    rest.starts_with("vimeo.com/") || rest.starts_with("www.vimeo.com/")
}

/// The title from an oEmbed answer, if it has a usable one.
pub fn parse_title(json: &serde_json::Value) -> Option<String> {
    let t = json.get("title")?.as_str()?.trim();
    (!t.is_empty()).then(|| t.chars().take(300).collect())
}

pub async fn title(url: &str) -> Option<String> {
    let resp = util::http()
        .get(endpoint(url)?)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    parse_title(&resp.json().await.ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_items_of_known_sites_only() {
        let yt = endpoint("https://www.youtube.com/watch?v=R6MlUcmOul8").unwrap();
        assert_eq!(yt, "https://www.youtube.com/oembed?format=json&url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3DR6MlUcmOul8");
        assert!(endpoint("https://youtu.be/abc").unwrap().starts_with("https://www.youtube.com/oembed"));
        assert!(endpoint("https://open.spotify.com/track/4PTG3Z6ehGkBFwjybzWkR8").unwrap().starts_with("https://open.spotify.com/oembed"));
        assert!(endpoint("https://vimeo.com/76979871").unwrap().starts_with("https://vimeo.com/api/oembed.json"));
        assert!(endpoint("https://www.tiktok.com/@u/video/1").is_some());
        assert!(endpoint("https://soundcloud.com/a/b").is_some());

        // Lists get their name from the list itself; a video URL inside a playlist
        // would name the wrong thing.
        assert_eq!(endpoint("https://www.youtube.com/watch?v=a&list=PL1"), None);
        assert_eq!(endpoint("https://soundcloud.com/a/sets/b"), None);
        assert_eq!(endpoint("https://notvimeo.com/1"), None);
        assert_eq!(endpoint("https://example.com/x"), None);
    }

    #[test]
    fn reads_the_title_field() {
        let j = serde_json::json!({ "title": "  Tears of Steel  ", "author_name": "Blender" });
        assert_eq!(parse_title(&j).as_deref(), Some("Tears of Steel"));
        assert_eq!(parse_title(&serde_json::json!({ "title": "" })), None);
        assert_eq!(parse_title(&serde_json::json!({ "html": "<iframe>" })), None);
        assert_eq!(parse_title(&serde_json::json!({ "title": "x".repeat(500) })).unwrap().len(), 300);
    }

    #[tokio::test]
    #[ignore = "braucht Netzwerk"]
    async fn e2e_youtube_title() {
        let t = title("https://www.youtube.com/watch?v=R6MlUcmOul8").await;
        assert_eq!(t.as_deref(), Some("Tears of Steel - Blender VFX Open Movie"));
    }
}
