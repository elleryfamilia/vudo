//! The embedded brand icon (the checkmark-`v` mark), materialized to a cache
//! file so the dialog backends that accept a custom icon — osascript on macOS,
//! zenity on Linux — can point at it.

use std::path::PathBuf;

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// Path to the brand icon on disk, writing it to the user cache dir on first
/// use. Returns None if it can't be written; dialogs then fall back to a stock
/// icon.
pub fn path() -> Option<String> {
    let mut file = cache_dir();
    std::fs::create_dir_all(&file).ok()?;
    file.push("icon.png");

    let stale = std::fs::metadata(&file)
        .map(|m| m.len() as usize != ICON_PNG.len())
        .unwrap_or(true);
    if stale {
        std::fs::write(&file, ICON_PNG).ok()?;
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
