use crate::deps::Tools;
use crate::engine::Shared;
use crate::job::{JobId, JobUpdate};
use crate::util;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

/// yt-dlp is video only. Photo posts (TikTok slideshows, Instagram photos,
/// X images) go through gallery-dl instead.
pub async fn download(
    shared: &Shared,
    id: JobId,
    tools: &Tools,
    url: &str,
    dir: &Path,
    cookies: Option<&str>,
    token: &CancellationToken,
) -> Result<PathBuf, String> {
    if !tools.gallery_dl().is_file() {
        return Err("gallery-dl fehlt (Tools aktualisieren)".into());
    }
    let target = dir.join(folder_name(url));
    if let Err(e) = tokio::fs::create_dir_all(&target).await {
        return Err(format!("Zielordner nicht anlegbar: {e}"));
    }

    let mut cmd = util::command(&tools.gallery_dl());
    cmd.args(["--no-colors", "-D"]).arg(&target);
    if let Some(b) = cookies {
        cmd.args(["--cookies-from-browser", b]);
    }
    cmd.arg("--").arg(url);

    let mut child = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return Err(format!("gallery-dl nicht startbar: {e}")),
    };
    let pid = child.id();
    let mut files = 0u32;
    let mut last_error = None;
    let mut last_warning = None;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(bool, String)>();
    if let Some(pipe) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(async move { forward(pipe, false, tx).await });
    }
    if let Some(pipe) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(async move { forward(pipe, true, tx).await });
    }
    drop(tx);

    loop {
        tokio::select! {
            _ = token.cancelled() => {
                if let Some(pid) = pid { util::kill_tree(pid); }
                let _ = child.wait().await;
                return Err("abgebrochen".into());
            }
            line = rx.recv() => {
                let Some((is_err, l)) = line else { break };
                if is_err {
                    if l.contains("[error]") || l.contains("Error") {
                        last_error = Some(l.trim().to_string());
                    } else if l.contains("[warning]") {
                        // Trägt meist den eigentlichen Grund (HTTP 429, 403 …).
                        last_warning = Some(l.trim().to_string());
                    }
                } else if !l.trim().is_empty() && !l.starts_with('#') {
                    files += 1;
                    shared.emit(id, JobUpdate::Note(format!("{files} Dateien")));
                }
            }
        }
    }

    let ok = matches!(child.wait().await, Ok(s) if s.success());
    if files == 0 {
        let base = last_error.unwrap_or_else(|| {
            if ok { "nichts zum Herunterladen gefunden".into() } else { "gallery-dl ist fehlgeschlagen".into() }
        });
        return Err(match last_warning {
            Some(w) => format!("{base} ({w})"),
            None => base,
        });
    }
    Ok(target)
}

async fn forward<R: tokio::io::AsyncRead + Unpin>(
    pipe: R,
    is_err: bool,
    tx: tokio::sync::mpsc::UnboundedSender<(bool, String)>,
) {
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(l)) = lines.next_line().await {
        if tx.send((is_err, l)).is_err() {
            break;
        }
    }
}

/// A readable folder name derived from the URL: `<site>_<user>_<id>`.
fn folder_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/');
    let after_scheme = path.split("://").nth(1).unwrap_or(path);
    let host = after_scheme
        .split('/')
        .next()
        .unwrap_or("web")
        .trim_start_matches("www.")
        .split('.')
        .next()
        .unwrap_or("web");
    let segments: Vec<&str> = after_scheme
        .split('/')
        .skip(1)
        .filter(|s| !s.is_empty())
        .collect();
    let id = segments.last().copied().unwrap_or("post");
    let name = match segments.iter().find(|s| s.starts_with('@')) {
        Some(user) => format!("{host}_{}_{id}", user.trim_start_matches('@')),
        None => format!("{host}_{id}"),
    };
    util::sanitize_filename(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_folder_names() {
        assert_eq!(folder_name("https://www.tiktok.com/@user/photo/7391?is=1"), "tiktok_user_7391");
        assert_eq!(folder_name("https://www.instagram.com/p/AbC123/"), "instagram_AbC123");
    }
}
