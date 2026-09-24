use crate::spotify::Track;
use crate::util;
use anyhow::Result;
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, TagExt};
use lofty::probe::Probe;
use lofty::tag::Tag;
use std::path::Path;

/// Writes Spotify metadata (and cover art) onto the downloaded audio file.
pub async fn tag(path: &Path, track: &Track) -> Result<()> {
    let cover = match &track.cover {
        Some(url) => fetch_bytes(url).await,
        None => None,
    };

    let path = path.to_path_buf();
    let track = track.clone();
    tokio::task::spawn_blocking(move || write_tags(&path, &track, cover)).await?
}

async fn fetch_bytes(url: &str) -> Option<Vec<u8>> {
    let resp = util::http().get(url).send().await.ok()?.error_for_status().ok()?;
    Some(resp.bytes().await.ok()?.to_vec())
}

fn write_tags(path: &Path, track: &Track, cover: Option<Vec<u8>>) -> Result<()> {
    let mut file = Probe::open(path)?.read()?;
    if file.primary_tag_mut().is_none() {
        let kind = file.primary_tag_type();
        file.insert_tag(Tag::new(kind));
    }
    let Some(tag) = file.primary_tag_mut() else {
        return Ok(());
    };

    tag.set_title(track.title.clone());
    if !track.artists.is_empty() {
        tag.set_artist(track.artist_line());
    }
    if let Some(album) = &track.album {
        tag.set_album(album.clone());
    }
    if let Some(n) = track.number {
        tag.set_track(n);
    }
    if let Some(bytes) = cover {
        let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            MimeType::Png
        } else {
            MimeType::Jpeg
        };
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(
            Picture::unchecked(bytes)
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build(),
        );
    }
    tag.save_to_path(path, WriteOptions::default())?;
    Ok(())
}
