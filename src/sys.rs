//! Platform specifics: child processes that die with the app, the `omnidl://`
//! link handler, and starting programs that must outlive the app.

#[cfg(all(unix, not(target_os = "macos")))]
use std::path::PathBuf;
use std::path::Path;
use std::process::Command;

/// Puts the app into a job object that kills every process it starts (yt-dlp,
/// ffmpeg, …) when the app exits, even after a crash. Otherwise a download
/// would keep running without a window and collide with its resumed twin.
#[cfg(windows)]
pub fn kill_children_on_exit() {
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok != 0 {
            AssignProcessToJobObject(job, GetCurrentProcess());
        }
        // The handle stays open for the life of the process; closing it at
        // exit is what ends the children.
    }
}

/// Unix: when the app exits normally, kills the process groups of the tools
/// it started, their own children included. A crash is covered on Linux by
/// the parent-death signal `util::command` arms; on macOS tools running at a
/// crash finish on their own.
#[cfg(unix)]
pub fn kill_children_on_exit() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    extern "C" fn at_exit() {
        unix::kill_child_groups();
    }
    // SAFETY: registers a plain function; it runs on `exit()` and after `main` returns.
    ONCE.call_once(|| unsafe {
        libc::atexit(at_exit);
    });
}

#[cfg(not(any(windows, unix)))]
pub fn kill_children_on_exit() {}

/// Starts a program outside the app's job (Windows) or session (Unix) so it
/// survives the app (file manager windows, the new version after an update).
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // An outer job (terminal, IDE) may forbid leaving it; then start normally.
        if let Ok(child) = cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB).spawn() {
            return Ok(child);
        }
        cmd.creation_flags(0);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe. A new session gets no hangup from
        // our terminal, and the exit cleanup leaves it alone.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn()
}

/// Registers `omnidl://` for the current user so the browser extension can
/// start the app when it is not running.
#[cfg(windows)]
pub fn register_link_handler(exe: &Path) -> std::io::Result<()> {
    let command = format!("\"{}\" \"%1\"", exe.display());
    let base = format!(r"Software\Classes\{}", crate::launch::SCHEME);
    set_value(&base, None, "URL:omnidl")?;
    set_value(&base, Some("URL Protocol"), "")?;
    set_value(&format!(r"{base}\DefaultIcon"), None, &format!("\"{}\",0", exe.display()))?;
    set_value(&format!(r"{base}\shell\open\command"), None, &command)
}

/// macOS learns `omnidl://` from the app bundle's Info.plist.
#[cfg(target_os = "macos")]
pub fn register_link_handler(_exe: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Registers `omnidl://` for the current user through a desktop entry in
/// `~/.local/share/applications`, which also lists omnidl in the app menu.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn register_link_handler(exe: &Path) -> std::io::Result<()> {
    use std::process::Stdio;
    let Some(data) = data_home() else { return Ok(()) };
    let apps = data.join("applications");
    std::fs::create_dir_all(&apps)?;
    // An AppImage runs from a temporary mount; the entry must name the image itself.
    let exe = std::env::var_os("APPIMAGE").map(PathBuf::from).unwrap_or_else(|| exe.to_path_buf());
    let icon = data.join("icons/hicolor/512x512/apps/omnidl.png");
    if !icon.is_file() {
        if let Some(dir) = icon.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&icon, include_bytes!("../assets/icon-512.png"))?;
    }
    let file = apps.join(DESKTOP_FILE);
    let entry = desktop_entry(&exe, &icon);
    if std::fs::read_to_string(&file).ok().as_deref() != Some(entry.as_str()) {
        std::fs::write(&file, entry)?;
        let _ = Command::new("update-desktop-database")
            .arg(&apps)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let status = Command::new("xdg-mime")
        .args(["default", DESKTOP_FILE, &format!("x-scheme-handler/{}", crate::launch::SCHEME)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() { Ok(()) } else { Err(std::io::Error::other("xdg-mime ist fehlgeschlagen")) }
}

#[cfg(all(unix, not(target_os = "macos")))]
const DESKTOP_FILE: &str = "omnidl.desktop";

/// `$XDG_DATA_HOME`, else `~/.local/share`.
#[cfg(all(unix, not(target_os = "macos")))]
fn data_home() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
}

/// Desktop entry that starts `exe` for `omnidl://` links.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn desktop_entry(exe: &Path, icon: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=omnidl\n\
         Comment={}\n\
         Exec={} %u\n\
         Icon={}\n\
         Terminal=false\n\
         Categories=AudioVideo;Network;\n\
         MimeType=x-scheme-handler/{};\n\
         StartupWMClass=omnidl\n",
        env!("CARGO_PKG_DESCRIPTION"),
        exec_arg(exe),
        icon.display(),
        crate::launch::SCHEME,
    )
}

/// Quotes a path for `Exec=`: inside double quotes `"`, `` ` ``, `$` and `\`
/// take a backslash, which in the file is written doubled; `%` becomes `%%`.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn exec_arg(path: &Path) -> String {
    let mut out = String::from('"');
    for c in path.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push_str(r"\\");
                out.push(c);
                if c == '\\' {
                    out.push('\\');
                }
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Finding and ending the tools at exit.
#[cfg(unix)]
mod unix {
    /// Tools `util::command` started lead their own process group inside our
    /// session; detached programs (`spawn_detached`) have their own session.
    pub fn kill_child_groups() {
        // SAFETY: plain syscalls on process ids.
        unsafe {
            let session = libc::getsid(0);
            for pid in child_pids(libc::getpid()) {
                if libc::getpgid(pid) == pid && libc::getsid(pid) == session {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn child_pids(parent: libc::pid_t) -> Vec<libc::pid_t> {
        let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
        dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .filter(|pid: &libc::pid_t| {
                std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| parent_of(&s)) == Some(parent)
            })
            .collect()
    }

    /// Parent pid from `/proc/<pid>/stat`: `pid (name) state ppid …`, where
    /// the name may itself hold spaces and parentheses.
    #[cfg(target_os = "linux")]
    pub(super) fn parent_of(stat: &str) -> Option<libc::pid_t> {
        let rest = &stat[stat.rfind(')')? + 1..];
        rest.split_whitespace().nth(1)?.parse().ok()
    }

    #[cfg(target_os = "macos")]
    pub(super) fn child_pids(parent: libc::pid_t) -> Vec<libc::pid_t> {
        let mut pids = vec![0 as libc::pid_t; 4096];
        let size = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer is as large as announced. Depending on the macOS
        // version the result counts pids or bytes; unused slots stay 0.
        let n = unsafe { libc::proc_listchildpids(parent, pids.as_mut_ptr().cast(), size) };
        pids.truncate(n.clamp(0, pids.len() as libc::c_int) as usize);
        pids.retain(|&p| p > 0);
        pids
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn child_pids(_parent: libc::pid_t) -> Vec<libc::pid_t> {
        Vec::new()
    }
}

#[cfg(windows)]
fn set_value(key: &str, name: Option<&str>, value: &str) -> std::io::Result<()> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let key = wide(key);
    let name = name.map(wide);
    let data = wide(value);
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    if status == 0 { Ok(()) } else { Err(std::io::Error::from_raw_os_error(status as i32)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kindprozesse eines Jobs mit KILL_ON_JOB_CLOSE dürfen trotzdem ausbrechen,
    /// sonst stürbe das neu gestartete omnidl mit dem alten.
    #[cfg(windows)]
    #[test]
    fn detached_processes_can_leave_the_job() {
        kill_children_on_exit();
        let mut c = Command::new("cmd");
        c.args(["/C", "exit 0"]);
        let status = spawn_detached(&mut c).expect("startet").wait().unwrap();
        assert!(status.success());
    }

    /// Losgelöste Programme bekommen eine eigene Sitzung, damit das Aufräumen
    /// beim Beenden sie verschont.
    #[cfg(unix)]
    #[test]
    fn detached_processes_get_their_own_session() {
        let mut c = Command::new("sleep");
        c.arg("5");
        let mut child = spawn_detached(&mut c).expect("startet");
        let pid = child.id() as libc::pid_t;
        // SAFETY: plain syscalls on our own child.
        let (session, ours) = unsafe { (libc::getsid(pid), libc::getsid(0)) };
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(session, pid, "eigene Sitzung");
        assert_ne!(session, ours);
    }

    /// Das Aufräumen beim Beenden findet die eigenen Kindprozesse.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn finds_own_children() {
        let mut child = Command::new("sleep").arg("5").spawn().expect("startet");
        let pids = unix::child_pids(std::process::id() as libc::pid_t);
        let _ = child.kill();
        let _ = child.wait();
        assert!(pids.contains(&(child.id() as libc::pid_t)), "{pids:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_parent_from_proc_stat() {
        assert_eq!(unix::parent_of("4242 (yt-dlp) S 17 4242 17 0 -1"), Some(17));
        assert_eq!(unix::parent_of("7 (a (b) c) R 1 7 7 0"), Some(1), "Klammern im Namen");
        assert_eq!(unix::parent_of("kaputt"), None);
    }

    #[test]
    fn desktop_entry_handles_omnidl_links() {
        let entry = desktop_entry(Path::new("/opt/omni dl/omnidl"), Path::new("/home/a/omnidl.png"));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=\"/opt/omni dl/omnidl\" %u\n"), "{entry}");
        assert!(entry.contains("\nMimeType=x-scheme-handler/omnidl;\n"));
        assert!(entry.contains("\nIcon=/home/a/omnidl.png\n"));
        assert!(entry.lines().all(|l| l.starts_with('[') || l.contains('=')), "nur Schlüssel=Wert");
    }

    /// Sonderzeichen im Pfad nach der Desktop-Entry-Spezifikation maskieren.
    #[test]
    fn exec_paths_are_escaped() {
        assert_eq!(exec_arg(Path::new("/usr/bin/omnidl")), r#""/usr/bin/omnidl""#);
        assert_eq!(exec_arg(Path::new(r#"/a$b"c`d"#)), r#""/a\\$b\\"c\\`d""#);
        assert_eq!(exec_arg(Path::new(r"/a\b")), r#""/a\\\\b""#);
        assert_eq!(exec_arg(Path::new("/100%")), r#""/100%%""#);
    }
}
