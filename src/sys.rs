//! Windows specifics: child processes that die with the app, the `omnidl://`
//! link handler, and starting programs that must outlive the app.

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

#[cfg(not(windows))]
pub fn kill_children_on_exit() {}

/// Starts a program outside the app's job so it survives the app (Explorer
/// windows, the new version after an update).
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

#[cfg(not(windows))]
pub fn register_link_handler(_exe: &Path) -> std::io::Result<()> {
    Ok(())
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Kindprozesse eines Jobs mit KILL_ON_JOB_CLOSE dürfen trotzdem ausbrechen,
    /// sonst stürbe das neu gestartete omnidl mit dem alten.
    #[test]
    fn detached_processes_can_leave_the_job() {
        kill_children_on_exit();
        let mut c = Command::new("cmd");
        c.args(["/C", "exit 0"]);
        let status = spawn_detached(&mut c).expect("startet").wait().unwrap();
        assert!(status.success());
    }
}
