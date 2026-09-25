use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Format {
    Mp4,
    Mkv,
    Mp3,
    M4a,
    Opus,
    Flac,
    Wav,
}

impl Format {
    pub const VIDEO: [Format; 2] = [Format::Mp4, Format::Mkv];
    pub const AUDIO: [Format; 5] = [Format::Mp3, Format::M4a, Format::Opus, Format::Flac, Format::Wav];

    pub fn label(self) -> &'static str {
        match self {
            Format::Mp4 => "MP4",
            Format::Mkv => "MKV",
            Format::Mp3 => "MP3",
            Format::M4a => "M4A",
            Format::Opus => "Opus",
            Format::Flac => "FLAC",
            Format::Wav => "WAV",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Mp4 => "mp4",
            Format::Mkv => "mkv",
            Format::Mp3 => "mp3",
            Format::M4a => "m4a",
            Format::Opus => "opus",
            Format::Flac => "flac",
            Format::Wav => "wav",
        }
    }

    pub fn is_audio(self) -> bool {
        !matches!(self, Format::Mp4 | Format::Mkv)
    }

    /// Formats where a bitrate/quality setting makes sense.
    pub fn has_bitrate(self) -> bool {
        matches!(self, Format::Mp3 | Format::M4a | Format::Opus)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum VideoQuality {
    Best,
    P2160,
    P1440,
    P1080,
    P720,
    P480,
    P360,
}

impl VideoQuality {
    pub const ALL: [VideoQuality; 7] = [
        VideoQuality::Best,
        VideoQuality::P2160,
        VideoQuality::P1440,
        VideoQuality::P1080,
        VideoQuality::P720,
        VideoQuality::P480,
        VideoQuality::P360,
    ];

    pub fn height(self) -> Option<u32> {
        match self {
            VideoQuality::Best => None,
            VideoQuality::P2160 => Some(2160),
            VideoQuality::P1440 => Some(1440),
            VideoQuality::P1080 => Some(1080),
            VideoQuality::P720 => Some(720),
            VideoQuality::P480 => Some(480),
            VideoQuality::P360 => Some(360),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoQuality::Best => "Beste Qualität",
            VideoQuality::P2160 => "2160p (4K)",
            VideoQuality::P1440 => "1440p",
            VideoQuality::P1080 => "1080p",
            VideoQuality::P720 => "720p",
            VideoQuality::P480 => "480p",
            VideoQuality::P360 => "360p",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum AudioQuality {
    Best,
    K320,
    K256,
    K192,
    K128,
}

impl AudioQuality {
    pub const ALL: [AudioQuality; 5] = [
        AudioQuality::Best,
        AudioQuality::K320,
        AudioQuality::K256,
        AudioQuality::K192,
        AudioQuality::K128,
    ];

    /// Value for yt-dlp `--audio-quality`.
    pub fn ytdlp_value(self) -> &'static str {
        match self {
            AudioQuality::Best => "0",
            AudioQuality::K320 => "320K",
            AudioQuality::K256 => "256K",
            AudioQuality::K192 => "192K",
            AudioQuality::K128 => "128K",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioQuality::Best => "Beste (VBR)",
            AudioQuality::K320 => "320 kbps",
            AudioQuality::K256 => "256 kbps",
            AudioQuality::K192 => "192 kbps",
            AudioQuality::K128 => "128 kbps",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Cookies {
    Off,
    Firefox,
    Edge,
    Chrome,
    Brave,
}

impl Cookies {
    pub const ALL: [Cookies; 5] = [
        Cookies::Off,
        Cookies::Firefox,
        Cookies::Edge,
        Cookies::Chrome,
        Cookies::Brave,
    ];

    pub fn browser(self) -> Option<&'static str> {
        match self {
            Cookies::Off => None,
            Cookies::Firefox => Some("firefox"),
            Cookies::Edge => Some("edge"),
            Cookies::Chrome => Some("chrome"),
            Cookies::Brave => Some("brave"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Cookies::Off => "Aus",
            Cookies::Firefox => "Firefox",
            Cookies::Edge => "Edge",
            Cookies::Chrome => "Chrome",
            Cookies::Brave => "Brave",
        }
    }
}

/// Video or audio, as requested from outside (browser extension, `omnidl://` link).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub download_dir: PathBuf,
    pub parallel: usize,
    pub format: Format,
    /// Formats to return to when switching between video and audio.
    pub last_video: Format,
    pub last_audio: Format,
    pub video_quality: VideoQuality,
    pub audio_quality: AudioQuality,
    pub playlist: bool,
    pub cookies: Cookies,
    pub check_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download_dir: downloads_dir(&env_path, std::env::consts::OS).join("omnidl"),
            parallel: 4,
            format: Format::Mp4,
            last_video: Format::Mp4,
            last_audio: Format::Mp3,
            video_quality: VideoQuality::Best,
            audio_quality: AudioQuality::Best,
            playlist: true,
            cookies: Cookies::Off,
            check_updates: true,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    pub fn save(&self) {
        self.save_to(&config_path());
    }

    fn load_from(path: &Path) -> Self {
        let mut cfg: Config = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        // Configs from 0.1 know no remembered formats; start from the current one.
        if cfg.format.is_audio() {
            cfg.last_audio = cfg.format;
        } else {
            cfg.last_video = cfg.format;
        }
        cfg
    }

    fn save_to(&self, path: &Path) {
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(path, s);
        }
    }

    /// Switches between video and audio, remembering the format of the side left.
    pub fn set_audio(&mut self, audio: bool) {
        if audio == self.format.is_audio() {
            return;
        }
        self.format = if audio { self.last_audio } else { self.last_video };
    }

    pub fn set_format(&mut self, f: Format) {
        self.format = f;
        if f.is_audio() {
            self.last_audio = f;
        } else {
            self.last_video = f;
        }
    }

    /// Copy for a download requested as video or audio from outside the window.
    pub fn for_mode(&self, mode: Option<Mode>) -> Config {
        let mut c = self.clone();
        if let Some(m) = mode {
            c.set_audio(m == Mode::Audio);
        }
        c
    }
}

/// A non-empty environment variable as a path.
fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The user's download folder: `~/Downloads`, on Linux the localized one from
/// `user-dirs.dirs` (`~/Téléchargements`) when set.
fn downloads_dir(var: &dyn Fn(&str) -> Option<PathBuf>, os: &str) -> PathBuf {
    let home = var(if os == "windows" { "USERPROFILE" } else { "HOME" }).unwrap_or_else(|| PathBuf::from("."));
    if os == "linux" {
        let config = var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let listed = std::fs::read_to_string(config.join("user-dirs.dirs")).ok();
        if let Some(dir) = listed.and_then(|s| xdg_download_dir(&s, &home)) {
            return dir;
        }
    }
    home.join("Downloads")
}

/// `XDG_DOWNLOAD_DIR="$HOME/Downloads"` from `user-dirs.dirs`. A value of just
/// `$HOME` means the folder is switched off.
fn xdg_download_dir(listing: &str, home: &Path) -> Option<PathBuf> {
    let value = listing.lines().find_map(|l| l.trim().strip_prefix("XDG_DOWNLOAD_DIR="))?.trim().trim_matches('"');
    let dir = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None if value.starts_with('/') => PathBuf::from(value),
        None => return None,
    };
    (dir != home).then_some(dir)
}

/// Where omnidl keeps its tools, settings and queue:
///
/// - `OMNIDL_HOME` if set (servers, containers);
/// - during development (exe inside `target/{debug,release}`) the Cargo
///   project root, so `bin/` is shared;
/// - Windows: next to the .exe (portable);
/// - macOS: `~/Library/Application Support/omnidl`;
/// - Linux and other Unix: `$XDG_DATA_HOME/omnidl`, else `~/.local/share/omnidl`.
pub fn base_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = locate_base(&exe_dir, &env_path, std::env::consts::OS);
    if dir != exe_dir {
        let _ = std::fs::create_dir_all(&dir);
    }
    dir
}

fn locate_base(exe_dir: &Path, var: &dyn Fn(&str) -> Option<PathBuf>, os: &str) -> PathBuf {
    if let Some(dir) = var("OMNIDL_HOME") {
        return dir;
    }
    let profile = exe_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let parent = exe_dir.parent();
    if matches!(profile, "debug" | "release")
        && parent.and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("target")
    {
        if let Some(root) = parent.and_then(|p| p.parent()) {
            return root.to_path_buf();
        }
    }
    let home = var("HOME");
    match os {
        "windows" => exe_dir.to_path_buf(),
        "macos" => home.map_or_else(|| exe_dir.to_path_buf(), |h| h.join("Library/Application Support/omnidl")),
        _ => var("XDG_DATA_HOME")
            .or_else(|| home.map(|h| h.join(".local/share")))
            .map_or_else(|| exe_dir.to_path_buf(), |d| d.join("omnidl")),
    }
}

fn config_path() -> PathBuf {
    base_dir().join("config.toml")
}

/// Everything the app writes next to itself. The uninstaller removes it;
/// the exe itself is the installer's business.
const APP_FILES: [&str; 7] =
    ["bin", "browser-extension", "config.toml", "queue.json", "queue.json.tmp", "omnidl.exe.old", "omnidl.exe.new"];

pub fn remove_app_files(base: &Path) {
    for name in APP_FILES {
        let path = base.join(name);
        let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
    }
    // The data folder itself, where nothing else is left in it.
    #[cfg(unix)]
    let _ = std::fs::remove_dir(base);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstall_removes_only_what_the_app_created() {
        let base = std::env::temp_dir().join("omnidl-tests").join("uninstall");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("bin").join("yt-dlp")).unwrap();
        std::fs::write(base.join("bin").join("yt-dlp").join("yt-dlp.exe"), "x").unwrap();
        for f in ["config.toml", "queue.json", "omnidl.exe", "fremd.txt"] {
            std::fs::write(base.join(f), "x").unwrap();
        }
        remove_app_files(&base);
        let mut left: Vec<String> = std::fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["fremd.txt", "omnidl.exe"]);
    }

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<PathBuf> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| PathBuf::from(v))
    }

    #[test]
    fn base_dir_per_platform() {
        let exe = Path::new("/opt/omnidl");
        let home = [("HOME", "/home/a")];
        assert_eq!(locate_base(exe, &env(&[]), "windows"), exe, "Windows bleibt portabel");
        assert_eq!(locate_base(exe, &env(&home), "macos"), Path::new("/home/a/Library/Application Support/omnidl"));
        assert_eq!(locate_base(exe, &env(&home), "linux"), Path::new("/home/a/.local/share/omnidl"));
        let xdg = [("HOME", "/home/a"), ("XDG_DATA_HOME", "/data")];
        assert_eq!(locate_base(exe, &env(&xdg), "linux"), Path::new("/data/omnidl"));
        assert_eq!(locate_base(exe, &env(&[]), "linux"), exe, "ohne HOME neben dem Programm");
        for os in ["windows", "macos", "linux"] {
            let vars = [("HOME", "/home/a"), ("OMNIDL_HOME", "/srv/omnidl")];
            assert_eq!(locate_base(exe, &env(&vars), os), Path::new("/srv/omnidl"), "OMNIDL_HOME gilt überall");
        }
        let dev = Path::new("/src/omnidl/target/debug");
        assert_eq!(locate_base(dev, &env(&home), "linux"), Path::new("/src/omnidl"), "Entwicklung nutzt das Projekt");
    }

    #[test]
    fn download_dir_per_platform() {
        let win = [("USERPROFILE", "C:/Users/a")];
        assert_eq!(downloads_dir(&env(&win), "windows"), Path::new("C:/Users/a/Downloads"));
        let mac = [("HOME", "/Users/a")];
        assert_eq!(downloads_dir(&env(&mac), "macos"), Path::new("/Users/a/Downloads"));
        let linux = [("HOME", "/home/a"), ("XDG_CONFIG_HOME", "/nirgends")];
        assert_eq!(downloads_dir(&env(&linux), "linux"), Path::new("/home/a/Downloads"));
    }

    #[test]
    fn reads_the_localized_download_folder() {
        let home = Path::new("/home/a");
        let listing = "# user-dirs\nXDG_DESKTOP_DIR=\"$HOME/Bureau\"\nXDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n";
        assert_eq!(xdg_download_dir(listing, home), Some(PathBuf::from("/home/a/Téléchargements")));
        assert_eq!(xdg_download_dir("XDG_DOWNLOAD_DIR=\"/mnt/dl\"", home), Some(PathBuf::from("/mnt/dl")));
        assert_eq!(xdg_download_dir("XDG_DOWNLOAD_DIR=\"$HOME/\"", home), None, "abgeschaltet");
        assert_eq!(xdg_download_dir("XDG_MUSIC_DIR=\"$HOME/Musik\"", home), None);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("omnidl-tests").join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn saves_and_loads_every_field() {
        let path = temp("roundtrip.toml");
        let cfg = Config {
            download_dir: PathBuf::from("D:/Musik"),
            parallel: 7,
            format: Format::Flac,
            last_video: Format::Mkv,
            last_audio: Format::Flac,
            video_quality: VideoQuality::P720,
            audio_quality: AudioQuality::K192,
            playlist: false,
            cookies: Cookies::Firefox,
            check_updates: false,
        };
        cfg.save_to(&path);
        assert_eq!(Config::load_from(&path), cfg);
    }

    /// Eine config.toml aus 0.1 kennt die neuen Felder nicht und muss trotzdem laden.
    #[test]
    fn reads_configs_from_version_0_1() {
        let path = temp("v01.toml");
        std::fs::write(
            &path,
            "download_dir = 'C:/dl'\nparallel = 3\nformat = 'Opus'\nvideo_quality = 'P1080'\n\
             audio_quality = 'Best'\nplaylist = true\ncookies = 'Off'\n",
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.parallel, 3);
        assert_eq!(cfg.format, Format::Opus);
        assert_eq!(cfg.last_audio, Format::Opus, "aktuelles Format wird gemerkt");
        assert_eq!(cfg.last_video, Format::Mp4);
        assert!(cfg.check_updates);
    }

    #[test]
    fn broken_config_falls_back_to_defaults() {
        let path = temp("broken.toml");
        std::fs::write(&path, "parallel = 'viele'").unwrap();
        assert_eq!(Config::load_from(&path).parallel, Config::default().parallel);
        assert_eq!(Config::load_from(&temp("missing.toml")).format, Format::Mp4);
    }

    #[test]
    fn switching_kind_returns_to_the_last_format() {
        let mut cfg = Config::default();
        cfg.set_format(Format::Mkv);
        cfg.set_audio(true);
        assert_eq!(cfg.format, Format::Mp3);
        cfg.set_format(Format::Flac);
        cfg.set_audio(false);
        assert_eq!(cfg.format, Format::Mkv);
        cfg.set_audio(true);
        assert_eq!(cfg.format, Format::Flac);
    }

    #[test]
    fn mode_from_outside_overrides_only_the_kind() {
        let mut cfg = Config::default();
        cfg.set_format(Format::Opus);
        cfg.set_audio(false);
        assert_eq!(cfg.for_mode(Some(Mode::Audio)).format, Format::Opus);
        assert_eq!(cfg.for_mode(Some(Mode::Video)).format, Format::Mp4);
        assert_eq!(cfg.for_mode(None).format, Format::Mp4);
        assert_eq!(cfg.format, Format::Mp4, "Original bleibt unverändert");
    }
}
