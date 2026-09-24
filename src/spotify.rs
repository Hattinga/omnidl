use crate::detect::SpotifyKind;
use crate::util;
use anyhow::{Context, Result, anyhow};
use serde_json::Value;

/// Spotify audio itself is DRM protected. We only read the public metadata from
/// the embed page (no API key needed) and let the matcher find the song elsewhere.
#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub duration_s: f64,
    pub album: Option<String>,
    pub cover: Option<String>,
    pub number: Option<u32>,
}

impl Track {
    pub fn artist_line(&self) -> String {
        self.artists.join(", ")
    }

    /// Search query for YouTube / YouTube Music.
    pub fn query(&self) -> String {
        let a = self.artists.first().map(|s| s.as_str()).unwrap_or("");
        format!("{a} {}", self.title).trim().to_string()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub name: String,
    pub kind: &'static str,
    pub tracks: Vec<Track>,
    pub cover: Option<String>,
    /// The embed page caps lists at 100 entries.
    pub truncated: bool,
}

pub async fn fetch(kind: SpotifyKind, id: &str) -> Result<Collection> {
    let url = format!("https://open.spotify.com/embed/{}/{}", kind.path(), id);
    let html = util::http()
        .get(&url)
        .send()
        .await
        .context("Spotify nicht erreichbar")?
        .error_for_status()
        .context("Spotify-Seite nicht abrufbar (privat oder gelöscht?)")?
        .text()
        .await?;
    parse_embed(&html)
}

/// Fetches a single track's details (cover art, proper artist list).
pub async fn track_details(id: &str) -> Result<Track> {
    let c = fetch(SpotifyKind::Track, id).await?;
    c.tracks.into_iter().next().ok_or_else(|| anyhow!("kein Track in der Antwort"))
}

pub fn parse_embed(html: &str) -> Result<Collection> {
    let start = html
        .find("__NEXT_DATA__")
        .and_then(|i| html[i..].find('>').map(|j| i + j + 1))
        .ok_or_else(|| anyhow!("Spotify-Seite hat ein unbekanntes Format"))?;
    let end = html[start..]
        .find("</script>")
        .ok_or_else(|| anyhow!("Spotify-Seite unvollständig"))?;
    let json: Value = serde_json::from_str(html[start..start + end].trim())
        .context("Spotify-Daten nicht lesbar")?;

    let entity = json
        .pointer("/props/pageProps/state/data/entity")
        .ok_or_else(|| anyhow!("keine Trackdaten gefunden"))?;

    let name = entity.get("name").and_then(Value::as_str).unwrap_or("Spotify").to_string();
    let cover = best_image(entity);
    let kind = entity.get("type").and_then(Value::as_str).unwrap_or("");

    if kind == "track" {
        let mut t = track_from_entity(entity);
        t.cover = cover.clone();
        return Ok(Collection {
            name: t.title.clone(),
            kind: "track",
            tracks: vec![t],
            cover,
            truncated: false,
        });
    }

    let list = entity
        .get("trackList")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("keine Trackliste gefunden"))?;

    let is_album = kind == "album";
    let tracks: Vec<Track> = list
        .iter()
        .enumerate()
        .map(|(i, t)| Track {
            id: t
                .get("uri")
                .and_then(Value::as_str)
                .and_then(|u| u.rsplit(':').next())
                .unwrap_or("")
                .to_string(),
            title: t.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
            artists: t
                .get("subtitle")
                .and_then(Value::as_str)
                .map(split_artists)
                .unwrap_or_default(),
            duration_s: t.get("duration").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0,
            album: is_album.then(|| name.clone()),
            cover: is_album.then(|| cover.clone()).flatten(),
            number: Some(i as u32 + 1),
        })
        .filter(|t| !t.title.is_empty())
        .collect();

    Ok(Collection {
        truncated: tracks.len() >= 100,
        name,
        kind: if is_album { "album" } else { "playlist" },
        tracks,
        cover,
    })
}

fn track_from_entity(entity: &Value) -> Track {
    Track {
        id: entity.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        title: entity.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        artists: entity
            .get("artists")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("name").and_then(Value::as_str))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default(),
        duration_s: entity.get("duration").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0,
        album: None,
        cover: None,
        number: None,
    }
}

/// Largest image from `visualIdentity.image`, else the plain cover art.
fn best_image(entity: &Value) -> Option<String> {
    let imgs = entity.pointer("/visualIdentity/image").and_then(Value::as_array);
    if let Some(imgs) = imgs {
        let best = imgs
            .iter()
            .max_by_key(|i| i.get("maxWidth").and_then(Value::as_u64).unwrap_or(0))?;
        return best.get("url").and_then(Value::as_str).map(|s| s.to_string());
    }
    entity
        .pointer("/coverArt/sources/0/url")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

/// `"Pitbull, Sensato"` -> two artists. Names containing a comma (e.g.
/// "Tyler, The Creator") are fixed later from the per-track details.
fn split_artists(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playlist_fixture() {
        let html = include_str!("../tests/fixtures/spotify_embed_playlist.html");
        let c = parse_embed(html).expect("playlist parst");
        assert_eq!(c.kind, "playlist");
        assert_eq!(c.name, "Today’s Top Hits");
        assert_eq!(c.tracks.len(), 50);
        assert!(!c.truncated);
        let t = &c.tracks[0];
        assert_eq!(t.title, "Bass Persuades");
        assert_eq!(t.artists, vec!["Miley Cyrus"]);
        assert!((t.duration_s - 202.46).abs() < 0.01);
        assert_eq!(t.number, Some(1));
        assert!(c.cover.as_deref().is_some_and(|u| u.starts_with("https://")));
        // playlist items have no per-track album
        assert!(t.album.is_none());
    }

    #[test]
    fn parses_track_fixture() {
        let html = include_str!("../tests/fixtures/spotify_embed_track.html");
        let c = parse_embed(html).expect("track parst");
        assert_eq!(c.kind, "track");
        assert_eq!(c.tracks.len(), 1);
        let t = &c.tracks[0];
        assert_eq!(t.title, "Bass Persuades");
        assert_eq!(t.artists, vec!["Miley Cyrus"]);
        assert_eq!(t.query(), "Miley Cyrus Bass Persuades");
        assert!(t.cover.is_some());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_embed("<html>nix</html>").is_err());
    }

    #[test]
    fn splits_artists() {
        assert_eq!(split_artists("Pitbull, Sensato"), vec!["Pitbull", "Sensato"]);
        assert_eq!(split_artists(""), Vec::<String>::new());
    }
}
