use crate::deps::Tools;
use crate::spotify::Track;
use crate::ytdlp;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub url: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<f64>,
    /// From the YouTube Music "songs" section (official audio, usually exact).
    pub from_music: bool,
}

/// Words that mean "not the track you asked for" unless the title says so.
const BAD_WORDS: [&str; 16] = [
    "live", "cover", "remix", "karaoke", "instrumental", "sped up", "slowed", "nightcore",
    "8d audio", "reverb", "acoustic", "reaction", "interview", "full album", "mashup", "tutorial",
];

/// Lowercases, drops bracketed noise and punctuation.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c.is_whitespace() => out.push(c.to_ascii_lowercase()),
            _ => out.push(' '),
        }
    }
    // Trailing decoration like "… (Official Video)" or "… Lyrics HD".
    const CUT: [&str; 9] = ["official", "video", "audio", "lyrics", "lyric", "visualizer", "hd", "hq", "mv"];
    let mut words: Vec<&str> = out.split_whitespace().collect();
    while words.len() > 1 && CUT.contains(words.last().unwrap()) {
        words.pop();
    }
    words.join(" ")
}

/// Title without a leading "artist - " prefix.
fn strip_artist<'a>(title: &'a str, artists: &[String]) -> &'a str {
    for a in artists {
        let a_norm = normalize(a);
        if !a_norm.is_empty() && title.starts_with(&a_norm) {
            return title[a_norm.len()..].trim_start();
        }
    }
    title
}

/// `None` means: reject this candidate.
pub fn score(track: &Track, c: &Candidate) -> Option<f64> {
    let want_title = normalize(&track.title);
    let cand_full = normalize(&c.title);
    let cand_title = strip_artist(&cand_full, &track.artists);

    let title_sim = strsim::jaro_winkler(&want_title, cand_title)
        .max(strsim::jaro_winkler(&want_title, &cand_full));
    if title_sim < 0.72 {
        return None;
    }

    // Duration: the strongest signal we have when it is known.
    let mut dur_score = 0.5;
    if let (Some(cand), true) = (c.duration, track.duration_s > 0.0) {
        let limit = (track.duration_s * 0.07).max(10.0);
        let diff = (cand - track.duration_s).abs();
        if diff > limit {
            return None;
        }
        dur_score = 1.0 - diff / limit;
    }

    let channel = c.channel.as_deref().unwrap_or("");
    let chan_norm = normalize(channel);
    let artist_hit = track.artists.iter().any(|a| {
        let a = normalize(a);
        !a.is_empty()
            && (chan_norm.contains(&a)
                || cand_full.contains(&a)
                || strsim::jaro_winkler(&chan_norm, &a) > 0.85)
    });

    let mut s = 0.45 * title_sim + 0.3 * f64::from(artist_hit) + 0.25 * dur_score;
    if c.from_music {
        s += 0.35;
    }
    if chan_norm.ends_with(" topic") {
        s += 0.1;
    }
    // Compared against the raw title: normalize() drops brackets, so a wanted
    // "(Live)" or "(Remix)" would otherwise look like an unwanted one.
    let want_raw = track.title.to_lowercase();
    for bad in BAD_WORDS {
        if cand_full.contains(bad) && !want_raw.contains(bad) {
            s -= 0.45;
        }
    }
    (s >= 0.5).then_some(s)
}

/// Best candidates first.
pub fn rank(track: &Track, cands: Vec<Candidate>) -> Vec<Candidate> {
    let mut scored: Vec<(f64, Candidate)> =
        cands.into_iter().filter_map(|c| score(track, &c).map(|s| (s, c))).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().map(|(_, c)| c).collect()
}

/// Queries YouTube Music and YouTube in parallel.
pub async fn search(tools: &Tools, track: &Track, cookies: Option<&str>) -> Vec<Candidate> {
    let q = track.query();
    let music_url = format!(
        "https://music.youtube.com/search?q={}#songs",
        urlencoding::encode(&q)
    );
    let yt = format!("ytsearch6:{q}");

    let (music, video) = tokio::join!(
        ytdlp::flat_list(tools, &music_url, Some(5), cookies),
        ytdlp::flat_list(tools, &yt, None, cookies),
    );

    let mut out = Vec::new();
    if let Ok((_, entries)) = music {
        out.extend(entries.into_iter().map(|e| Candidate {
            url: e.url,
            title: e.title,
            channel: e.channel,
            duration: e.duration,
            from_music: true,
        }));
    }
    if let Ok((_, entries)) = video {
        out.extend(entries.into_iter().map(|e| Candidate {
            url: e.url,
            title: e.title,
            channel: e.channel,
            duration: e.duration,
            from_music: false,
        }));
    }
    rank(track, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            id: "x".into(),
            title: "Bass Persuades".into(),
            artists: vec!["Miley Cyrus".into()],
            duration_s: 202.46,
            ..Default::default()
        }
    }

    fn cand(title: &str, dur: Option<f64>, ch: &str, music: bool) -> Candidate {
        Candidate {
            url: "u".into(),
            title: title.into(),
            channel: Some(ch.into()),
            duration: dur,
            from_music: music,
        }
    }

    #[test]
    fn normalizes_noise_away() {
        assert_eq!(normalize("Bass Persuades (Official Video)"), "bass persuades");
        assert_eq!(normalize("Miley Cyrus - Bass Persuades [Lyrics]"), "miley cyrus bass persuades");
        assert_eq!(normalize("Song Official Audio"), "song");
    }

    #[test]
    fn rejects_wrong_duration() {
        // the 4-minute music video of a 3:22 track
        assert!(score(&track(), &cand("Bass Persuades (Official Video)", Some(239.0), "MILEY", false)).is_none());
        // an interview
        assert!(score(&track(), &cand("Miley: Bass Persuades, Dolly Parton | Interview", Some(4191.0), "Apple Music", false)).is_none());
    }

    #[test]
    fn accepts_matching_audio() {
        let s = score(&track(), &cand("Miley Cyrus - Bass Persuades (Lyrics)", Some(203.0), "VibesOnly", false));
        assert!(s.is_some(), "lyrics upload mit passender Länge wird akzeptiert");
    }

    #[test]
    fn prefers_youtube_music_and_topic_channels() {
        let music = cand("Bass Persuades", None, "Miley Cyrus", true);
        let random = cand("Miley Cyrus - Bass Persuades (Lyrics)", Some(203.0), "VibesOnly", false);
        let ranked = rank(&track(), vec![random.clone(), music.clone()]);
        assert!(ranked[0].from_music, "YT-Music-Treffer steht vorne");
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn penalizes_remixes_and_covers() {
        let good = cand("Bass Persuades", Some(202.0), "Miley Cyrus - Topic", false);
        let remix = cand("Bass Persuades (Tiesto Remix)", Some(202.0), "Miley Cyrus", false);
        let sg = score(&track(), &good).unwrap();
        match score(&track(), &remix) {
            Some(sr) => assert!(sg > sr, "Original schlägt Remix"),
            None => {}
        }
    }

    #[test]
    fn remix_wanted_is_not_penalized() {
        let mut t = track();
        t.title = "Bass Persuades (Tiesto Remix)".into();
        // normalize() drops the bracket, so the wanted title must still match
        assert!(score(&t, &cand("Bass Persuades Tiesto Remix", Some(202.0), "Miley Cyrus", false)).is_some());
    }
}
