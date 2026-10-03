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
    materialize_in(&cache_dir(), ICON_PNG, "icon.png")
}

/// Same as [`path`], for the fingerprint glyph shown while a biometric PAM
/// module (pam_fprintd, pam_u2f) is waiting for a finger.
pub fn fingerprint() -> Option<String> {
    materialize_in(&cache_dir(), FINGERPRINT_PNG, "fingerprint.png")
}

/// Write an embedded icon to a cache dir, refreshing it whenever the embedded
/// bytes change. Staleness is decided by content hash (stored in a `<name>.v`
/// sidecar), not file size: an edit that keeps the size identical — a
/// recolor, most palette swaps — must still refresh. Returns None if it
/// can't be written.
fn materialize_in(dir: &std::path::Path, bytes: &[u8], name: &str) -> Option<String> {
    std::fs::create_dir_all(dir).ok()?;
    let icon = dir.join(name);
    let version = dir.join(format!("{name}.v"));

    let want = format!("{:016x}\n", fnv1a(bytes));
    let stale = std::fs::read_to_string(&version).ok().as_deref() != Some(want.as_str());
    if stale {
        std::fs::write(&icon, bytes).ok()?;
        std::fs::write(&version, want).ok()?;
    }
    icon.to_str().map(str::to_string)
}

/// FNV-1a 64: a few lines, no dependencies, only used to notice when the
/// embedded icon bytes changed — collision resistance doesn't matter here.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_known_vectors() {
        // Published test vectors for FNV-1a 64.
        assert_eq!(fnv1a(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn same_size_content_change_refreshes_the_cache() {
        // Scratch cache dir injected directly, not the process env: set_var
        // in a test races with parallel tests reading the environment.
        let dir = std::env::temp_dir().join(format!("vudo-icon-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Materialize, then "edit" the embedded bytes keeping the size
        // identical — the old size-only check would keep the stale file.
        let a = materialize_in(&dir, b"same-length-icon-v1", "glyph.png");
        std::fs::write(a.unwrap(), b"XXXXXXXXXXXXXXXXXX").unwrap(); // corrupt it
        let b = materialize_in(&dir, b"same-length-icon-v2", "glyph.png");
        let bytes = std::fs::read(b.unwrap()).unwrap();
        assert_eq!(
            bytes, b"same-length-icon-v2",
            "same-size change must refresh"
        );

        // Unchanged bytes: no rewrite (the corrupted file survives).
        std::fs::write(dir.join("glyph.png"), b"hand-edited").unwrap();
        let _ = materialize_in(&dir, b"same-length-icon-v2", "glyph.png");
        assert_eq!(
            std::fs::read(dir.join("glyph.png")).unwrap(),
            b"hand-edited",
            "unchanged bytes must not be rewritten"
        );

        // A garbled or missing sidecar must read as stale, not fresh.
        std::fs::write(dir.join("glyph.png.v"), b"nonsense").unwrap();
        let _ = materialize_in(&dir, b"same-length-icon-v2", "glyph.png");
        assert_eq!(
            std::fs::read(dir.join("glyph.png")).unwrap(),
            b"same-length-icon-v2",
            "garbled sidecar must trigger a refresh"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
