use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";

/// Child process without console window, UTF-8 output, killed when dropped.
/// On Unix it leads its own process group, so `kill_tree` also reaches what it
/// starts (ffmpeg, Deno); on Linux it also dies with omnidl, crash included.
pub fn command(program: &Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(program);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    #[cfg(unix)]
    c.process_group(0);
    #[cfg(target_os = "linux")]
    {
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            c.pre_exec(move || {
                // SIGTERM rather than SIGKILL: PyInstaller launchers pass it on.
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                if libc::getppid() != parent {
                    // omnidl ended before the signal was armed.
                    libc::raise(libc::SIGTERM);
                }
                Ok(())
            });
        }
    }
    c.env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    c
}

/// Kills a process and all its children (yt-dlp spawns ffmpeg).
#[cfg(windows)]
pub fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("taskkill");
    c.args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let _ = c.status();
}

/// Kills a process and all its children (yt-dlp spawns ffmpeg): the whole
/// process group `command` gave it.
#[cfg(unix)]
pub fn kill_tree(pid: u32) {
    let pid = pid as libc::pid_t;
    // SAFETY: plain syscalls; a negative pid addresses the group.
    unsafe {
        if libc::kill(-pid, libc::SIGKILL) != 0 {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

pub fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("http client")
    })
}

/// Makes a string safe as a Windows file name.
pub fn sanitize_filename(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > 150 {
        out = out.chars().take(150).collect();
    }
    let out = out.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if out.is_empty() { "download".into() } else { out }
}

/// Escapes a literal for use inside a yt-dlp output template.
pub fn escape_template(s: &str) -> String {
    s.replace('%', "%%")
}

/// Name of the file manager `reveal` opens, for labels.
pub const FILE_MANAGER: &str = if cfg!(windows) {
    "Explorer"
} else if cfg!(target_os = "macos") {
    "Finder"
} else {
    "Dateimanager"
};

/// Opens the file manager with the file selected (or the folder itself).
/// Linux file managers cannot be asked to select, so there the folder opens.
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("explorer");
        if path.is_file() {
            c.raw_arg(format!("/select,\"{}\"", path.display()));
        } else {
            c.raw_arg(format!("\"{}\"", path.display()));
        }
        c
    };
    #[cfg(target_os = "macos")]
    let mut c = {
        let mut c = std::process::Command::new("open");
        if path.is_file() {
            c.arg("-R");
        }
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut c = {
        let dir = if path.is_file() { path.parent().unwrap_or(path) } else { path };
        let mut c = std::process::Command::new("xdg-open");
        c.arg(dir).stdout(Stdio::null()).stderr(Stdio::null());
        c
    };
    // File manager windows stay open when omnidl quits.
    let child = crate::sys::spawn_detached(&mut c);
    // Reap the launcher once it returns, so no zombie is left behind.
    #[cfg(unix)]
    if let Ok(mut child) = child {
        std::thread::spawn(move || child.wait());
    }
    #[cfg(not(unix))]
    drop(child);
}

pub fn fmt_bytes(b: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{v:.0} {}", UNITS[i]) } else { format!("{v:.1} {}", UNITS[i]) }
}

pub fn fmt_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn today_days() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_filename("AC/DC: Back in Black?"), "AC_DC_ Back in Black_");
        assert_eq!(sanitize_filename("  ..  "), "download");
        assert_eq!(sanitize_filename("a\tb   c."), "a b c");
    }

    #[test]
    fn civil() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 9, 22), 20_718);
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_eta(65), "1:05");
        assert_eq!(fmt_eta(3725), "1:02:05");
        assert_eq!(fmt_bytes(1536.0), "1.5 KB");
    }

    /// Reads a pipe to its end in the background; `true` once every writer is gone.
    #[cfg(unix)]
    fn closes_within(mut pipe: impl std::io::Read + Send + 'static, secs: u64) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut rest = Vec::new();
            let _ = pipe.read_to_end(&mut rest);
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(secs)).is_ok()
    }

    /// Abbrechen beendet auch, was das Werkzeug selbst gestartet hat (yt-dlp → ffmpeg):
    /// solange ein Enkel lebt, bliebe die Pipe offen.
    #[cfg(unix)]
    #[tokio::test]
    async fn kill_tree_ends_grandchildren() {
        use tokio::io::AsyncBufReadExt;
        let mut child = command(Path::new("/bin/sh"))
            .args(["-c", "sleep 30 & echo los; wait"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("sh startet");
        let mut out = tokio::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).await.unwrap();
        assert_eq!(line.trim(), "los");
        kill_tree(child.id().unwrap());
        let _ = child.wait().await;
        let pipe = out.into_inner().into_owned_fd().unwrap();
        assert!(closes_within(std::fs::File::from(pipe), 5), "sleep lebt noch");
    }

    /// Wie das Job-Objekt unter Windows: Werkzeuge sterben mit omnidl. Das Signal
    /// hängt am startenden Thread, darum genügt hier ein Thread, der endet.
    #[cfg(target_os = "linux")]
    #[test]
    fn tools_die_with_omnidl() {
        let pipe = std::thread::spawn(|| {
            let mut c = command(Path::new("sleep"));
            c.arg("30").stdout(Stdio::piped());
            let mut child = c.as_std_mut().spawn().expect("sleep startet");
            child.stdout.take().unwrap()
        })
        .join()
        .unwrap();
        assert!(closes_within(pipe, 5), "sleep überlebt seinen Starter");
    }
}
