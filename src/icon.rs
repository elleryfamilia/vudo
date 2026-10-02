//! Embedded icons (the checkmark-`v` brand mark, the fingerprint glyph),
//! materialized to cache files so the dialog backends that accept a custom
//! icon — osascript on macOS, zenity/yad on Linux — can point at them.

use std::path::PathBuf;

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
const FINGERPRINT_PNG: &[u8] = include_bytes!("../assets/fingerprint.png");

/// Path to the brand icon on disk, writing it to the user cache dir on first
/// use. Returns None if it can't be written; dialogs then fall back to a stock
/// icon.
pub fn path() -> Option<String> {
    materialize(ICON_PNG, "icon.png")
}

/// Same as [`path`], for the fingerprint glyph shown while a biometric PAM
/// module (pam_fprintd, pam_u2f) is waiting for a finger.
pub fn fingerprint() -> Option<String> {
    materialize(FINGERPRINT_PNG, "fingerprint.png")
}

/// Write an embedded icon to the user cache dir on first use (or when the
/// embedded bytes change size), returning its path.
fn materialize(bytes: &[u8], name: &str) -> Option<String> {
    let mut file = cache_dir();
    std::fs::create_dir_all(&file).ok()?;
    file.push(name);

    let stale = std::fs::metadata(&file)
        .map(|m| m.len() as usize != bytes.len())
        .unwrap_or(true);
    if stale {
        std::fs::write(&file, bytes).ok()?;
    }
    file.to_str().map(str::to_string)
}

fn cache_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return [x.as_str(), "vudo"].iter().collect();
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return [home.as_str(), ".cache", "vudo"].iter().collect();
    }
    std::env::temp_dir()
}
