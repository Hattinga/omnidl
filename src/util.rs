use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";

/// Child process without console window, UTF-8 output, killed when dropped.
pub fn command(program: &Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(program);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c.env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    c
}

/// Kills a process and all its children (yt-dlp spawns ffmpeg).
pub fn kill_tree(pid: u32) {
    let mut c = std::process::Command::new("taskkill");
    c.args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = c.status();
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

/// Opens Explorer with the file selected (or the folder itself).
pub fn reveal(path: &Path) {
    let mut c = std::process::Command::new("explorer");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if path.is_file() {
            c.raw_arg(format!("/select,\"{}\"", path.display()));
        } else {
            c.raw_arg(format!("\"{}\"", path.display()));
        }
    }
    #[cfg(not(windows))]
    c.arg(path);
    // Explorer windows stay open when omnidl quits.
    let _ = crate::sys::spawn_detached(&mut c);
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
}
