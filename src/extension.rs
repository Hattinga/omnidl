//! The browser extension travels inside the exe and is written out when the
//! user wants to install it, so omnidl stays a single file.

use std::path::{Path, PathBuf};

const FILES: &[(&str, &[u8])] = &[
    ("manifest.json", include_bytes!("../extension/manifest.json")),
    ("omnidl.js", include_bytes!("../extension/omnidl.js")),
    ("background.js", include_bytes!("../extension/background.js")),
    ("popup.html", include_bytes!("../extension/popup.html")),
    ("popup.css", include_bytes!("../extension/popup.css")),
    ("popup.js", include_bytes!("../extension/popup.js")),
    ("icons/16.png", include_bytes!("../extension/icons/16.png")),
    ("icons/32.png", include_bytes!("../extension/icons/32.png")),
    ("icons/48.png", include_bytes!("../extension/icons/48.png")),
    ("icons/128.png", include_bytes!("../extension/icons/128.png")),
];

pub fn dir(base: &Path) -> PathBuf {
    base.join("browser-extension")
}

/// Writes the extension next to the app (only files that changed, so a
/// browser that has it loaded sees no needless reload) and returns the folder.
pub fn export(base: &Path) -> std::io::Result<PathBuf> {
    let dir = dir(base);
    for (name, bytes) in FILES {
        let path = dir.join(name);
        if std::fs::read(&path).is_ok_and(|b| b == *bytes) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_a_loadable_extension() {
        let base = std::env::temp_dir().join("omnidl-tests").join("ext");
        let _ = std::fs::remove_dir_all(&base);
        let dir = export(&base).unwrap();
        for (name, bytes) in FILES {
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), *bytes, "{name}");
        }
        // Zweiter Export ändert nichts und scheitert nicht.
        export(&base).unwrap();
    }

    /// Manifest und App müssen zusammenpassen: gleiche Version, gleicher Port.
    #[test]
    fn manifest_matches_the_app() {
        let manifest: serde_json::Value = serde_json::from_slice(FILES[0].1).unwrap();
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(manifest["manifest_version"], 3);
        let host = manifest["host_permissions"][0].as_str().unwrap();
        assert_eq!(host, format!("http://127.0.0.1:{}/*", crate::bridge::PORT));

        let api = std::str::from_utf8(FILES[1].1).unwrap();
        assert!(api.contains(&format!("127.0.0.1:{}", crate::bridge::PORT)));
        assert!(api.to_ascii_lowercase().contains(crate::bridge::CLIENT_HEADER));
        assert!(api.contains(&format!("{}://add?", crate::launch::SCHEME)));

        // Every file the manifest and popup reference is shipped.
        let shipped: Vec<&str> = FILES.iter().map(|(n, _)| *n).collect();
        let popup = std::str::from_utf8(FILES[3].1).unwrap();
        for referenced in ["popup.css", "omnidl.js", "popup.js", "icons/32.png"] {
            assert!(popup.contains(referenced), "{referenced} im Popup");
            assert!(shipped.contains(&referenced), "{referenced} mitgeliefert");
        }
        for icon in manifest["icons"].as_object().unwrap().values() {
            assert!(shipped.contains(&icon.as_str().unwrap()));
        }
    }
}
