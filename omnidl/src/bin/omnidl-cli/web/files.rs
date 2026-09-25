//! Finished downloads onto the device in front of the browser. Only paths the
//! engine reported as a job's output are ever opened; requests name a job id,
//! never a path. Folders (photo posts) and playlists arrive as one ZIP.

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const CHUNK: usize = 64 * 1024;

/// What a job has to offer for download.
#[derive(Debug, PartialEq)]
pub enum Offer {
    File(PathBuf),
    /// A ZIP named `name.zip` holding `(name inside the archive, file)`.
    Zip { name: String, entries: Vec<(String, PathBuf)> },
}

/// Every regular file below `dir`, with its path relative to `dir`. Links are
/// not followed, so nothing outside the folder can end up in the archive.
pub fn walk(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, prefix)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else { continue };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(kind) = e.file_type() else { continue };
            let name = format!("{prefix}{}", e.file_name().to_string_lossy());
            if kind.is_dir() {
                stack.push((e.path(), format!("{name}/")));
            } else if kind.is_file() {
                out.push((name, e.path()));
            }
        }
    }
    out.sort();
    out
}

/// Names inside an archive must be unique; a second `a.mp3` becomes `a (2).mp3`.
pub fn unique_names(entries: Vec<(String, PathBuf)>) -> Vec<(String, PathBuf)> {
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .map(|(name, path)| {
            let mut candidate = name.clone();
            let mut n = 2;
            while !seen.insert(candidate.to_lowercase()) {
                candidate = match name.rsplit_once('.') {
                    Some((stem, ext)) => format!("{stem} ({n}).{ext}"),
                    None => format!("{name} ({n})"),
                };
                n += 1;
            }
            (candidate, path)
        })
        .collect()
}

pub async fn respond(offer: Offer, headers: &HeaderMap) -> Response {
    match offer {
        Offer::File(path) => send_file(&path, headers).await,
        Offer::Zip { name, entries } => send_zip(&name, entries),
    }
}

async fn send_file(path: &Path, headers: &HeaderMap) -> Response {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return super::api::error(StatusCode::NOT_FOUND, "Die Datei ist nicht mehr vorhanden.");
    };
    let len = match file.metadata().await {
        Ok(m) if m.is_file() => m.len(),
        _ => return super::api::error(StatusCode::NOT_FOUND, "Die Datei ist nicht mehr vorhanden."),
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "download".into());
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(|v| parse_range(v, len));

    let (status, start, count) = match range {
        None | Some(Range::Ignore) => (StatusCode::OK, 0, len),
        Some(Range::Part(start, end)) => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        Some(Range::Unsatisfiable) => {
            let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            resp.headers_mut().insert(header::CONTENT_RANGE, value(&format!("bytes */{len}")));
            return resp;
        }
    };
    if start > 0 && file.seek(io::SeekFrom::Start(start)).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let body = stream::unfold((file, count), |(mut file, left)| async move {
        if left == 0 {
            return None;
        }
        let mut buf = vec![0u8; CHUNK.min(left as usize)];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, io::Error>(Bytes::from(buf)), (file, left - n as u64)))
            }
            Err(e) => Some((Err(e), (file, 0))),
        }
    });
    let mut resp = Response::new(Body::from_stream(body));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, value(content_type(&name)));
    h.insert(header::CONTENT_LENGTH, value(&count.to_string()));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_DISPOSITION, disposition(&name));
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(header::CONTENT_RANGE, value(&format!("bytes {start}-{}/{len}", start + count - 1)));
    }
    resp
}

/// Streams a ZIP built on the fly. Media is already compressed, so entries are
/// stored as they are: fast, and the server needs no scratch space.
fn send_zip(name: &str, entries: Vec<(String, PathBuf)>) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<io::Result<Bytes>>(4);
    tokio::task::spawn_blocking(move || {
        let err = tx.clone();
        if let Err(e) = write_zip(entries, Pipe { tx, buf: Vec::with_capacity(CHUNK) }) {
            // Ends the download with an error instead of a truncated archive that looks complete.
            let _ = err.blocking_send(Err(e));
        }
    });
    let body = stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|chunk| (chunk, rx)) });
    let mut resp = Response::new(Body::from_stream(body));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    h.insert(header::CONTENT_DISPOSITION, disposition(&format!("{name}.zip")));
    resp
}

pub fn write_zip(entries: Vec<(String, PathBuf)>, out: impl Write) -> io::Result<()> {
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new_stream(out);
    for (name, path) in entries {
        let mut file = std::fs::File::open(&path)?;
        let size = file.metadata()?.len();
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .large_file(size >= u32::MAX as u64);
        zip.start_file(name, opts).map_err(io::Error::other)?;
        io::copy(&mut file, &mut zip)?;
    }
    zip.finish().map_err(io::Error::other)?.into_inner().flush()
}

/// Blocking writer feeding the response body in chunks.
struct Pipe {
    tx: tokio::sync::mpsc::Sender<io::Result<Bytes>>,
    buf: Vec<u8>,
}

impl Pipe {
    fn send(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = Bytes::from(std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK)));
        // A closed channel means the browser went away; stop reading files.
        self.tx.blocking_send(Ok(chunk)).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

impl Write for Pipe {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.send()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()
    }
}

#[derive(Debug, PartialEq)]
pub enum Range {
    /// Inclusive byte range.
    Part(u64, u64),
    /// Several ranges or something unreadable: send the whole file.
    Ignore,
    Unsatisfiable,
}

/// `bytes=0-99`, `bytes=100-`, `bytes=-500`. One range only.
pub fn parse_range(v: &str, len: u64) -> Range {
    let Some(spec) = v.trim().strip_prefix("bytes=") else { return Range::Ignore };
    if spec.contains(',') {
        return Range::Ignore;
    }
    let Some((a, b)) = spec.trim().split_once('-') else { return Range::Ignore };
    let (a, b) = (a.trim(), b.trim());
    let parsed = match (a.is_empty(), b.is_empty()) {
        (false, _) => {
            let Ok(start) = a.parse::<u64>() else { return Range::Ignore };
            let end = if b.is_empty() { Ok(u64::MAX) } else { b.parse::<u64>() };
            let Ok(end) = end else { return Range::Ignore };
            if end < start {
                return Range::Ignore;
            }
            (start, end.min(len.saturating_sub(1)))
        }
        (true, false) => {
            let Ok(suffix) = b.parse::<u64>() else { return Range::Ignore };
            if suffix == 0 {
                return Range::Unsatisfiable;
            }
            (len.saturating_sub(suffix), len.saturating_sub(1))
        }
        (true, true) => return Range::Ignore,
    };
    if parsed.0 >= len { Range::Unsatisfiable } else { Range::Part(parsed.0, parsed.1) }
}

/// `attachment` with an ASCII fallback and the real name in UTF-8 (RFC 6266).
pub fn disposition(name: &str) -> HeaderValue {
    let ascii: String = name
        .chars()
        .map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' })
        .collect();
    value(&format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{}", urlencoding::encode(name)))
}

fn value(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
}

pub fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "opus" | "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Range::Part(0, 99));
        assert_eq!(parse_range("bytes=900-", 1000), Range::Part(900, 999));
        assert_eq!(parse_range("bytes=-100", 1000), Range::Part(900, 999));
        assert_eq!(parse_range("bytes=-5000", 1000), Range::Part(0, 999));
        assert_eq!(parse_range("bytes=990-2000", 1000), Range::Part(990, 999));
        assert_eq!(parse_range("bytes=1000-", 1000), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 1000), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), Range::Ignore);
        assert_eq!(parse_range("bytes=5-1", 1000), Range::Ignore);
        assert_eq!(parse_range("items=0-1", 1000), Range::Ignore);
        assert_eq!(parse_range("bytes=x-1", 1000), Range::Ignore);
    }

    #[test]
    fn filenames_survive_in_utf8() {
        let d = disposition("Björk – Jóga \"live\".mp3");
        let s = d.to_str().unwrap();
        assert!(s.starts_with("attachment; filename=\"Bj_rk _ J_ga _live_.mp3\""), "{s}");
        assert!(s.contains("filename*=UTF-8''Bj%C3%B6rk%20%E2%80%93%20J%C3%B3ga%20%22live%22.mp3"), "{s}");
        assert!(disposition("a\r\nb.mp4").to_str().unwrap().contains("filename=\"a__b.mp4\""));
    }

    #[test]
    fn media_types() {
        assert_eq!(content_type("x.MP4"), "video/mp4");
        assert_eq!(content_type("x.opus"), "audio/ogg");
        assert_eq!(content_type("x.html"), "application/octet-stream", "nichts, was der Browser ausführt");
        assert_eq!(content_type("ohne"), "application/octet-stream");
    }

    #[test]
    fn duplicate_names_get_numbered() {
        let e = |n: &str| (n.to_string(), PathBuf::from(n));
        let names: Vec<String> =
            unique_names(vec![e("a.mp3"), e("A.mp3"), e("b"), e("b"), e("a.mp3")]).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["a.mp3", "A (2).mp3", "b", "b (2)", "a (3).mp3"]);
    }

    #[test]
    fn folders_zip_with_their_structure() {
        let dir = std::env::temp_dir().join("omnidl-tests").join("web-zip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("1.jpg"), vec![7u8; 100_000]).unwrap();
        std::fs::write(dir.join("sub").join("2.jpg"), b"zwei").unwrap();
        let entries = walk(&dir);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["1.jpg", "sub/2.jpg"]);

        let mut buf = Vec::new();
        write_zip(entries, &mut buf).unwrap();
        let mut archive = zip::ZipArchive::new(io::Cursor::new(buf)).unwrap();
        assert_eq!(archive.len(), 2);
        let mut data = Vec::new();
        io::Read::read_to_end(&mut archive.by_name("sub/2.jpg").unwrap(), &mut data).unwrap();
        assert_eq!(data, b"zwei");
        assert_eq!(archive.by_name("1.jpg").unwrap().size(), 100_000);
    }
}
