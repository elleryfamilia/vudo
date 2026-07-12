//! macOS dialogs via osascript, plus Touch ID detection.

use std::process::{Command, Stdio};

/// True if `pam_tid` (Touch ID for sudo) is enabled in an uncommented auth
/// line of the sudo PAM config.
pub fn has_touch_id() -> bool {
    for f in ["/etc/pam.d/sudo_local", "/etc/pam.d/sudo"] {
        if let Ok(contents) = std::fs::read_to_string(f) {
            for line in contents.lines() {
                let line = line.trim_start();
                if line.starts_with('#') {
                    continue;
                }
                if line.starts_with("auth") && line.contains("pam_tid.so") {
                    return true;
                }
            }
        }
    }
    false
}

/// Preview-only confirmation dialog (used ahead of the Touch ID sheet).
pub fn confirm(preview: &str) -> bool {
    let script = format!(
        "display dialog \"vudo will run this command as root:\" & return & return & {} \
         with title \"vudo\" with icon caution \
         buttons {{\"Cancel\", \"Run as root\"}} default button \"Run as root\"",
        apple_str(preview)
    );
    Command::new("osascript")
        .args(["-e", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Password dialog that also previews the command. Returns None on cancel.
pub fn ask_password(preview: &str) -> Option<String> {
    let script = format!(
        "display dialog \"vudo will run this command as root:\" & return & return & {} \
         & return & return & \"Enter your password to authorize.\" \
         with title \"vudo\" with icon caution \
         default answer \"\" with hidden answer \
         buttons {{\"Cancel\", \"Run as root\"}} default button \"Run as root\"\n\
         return text returned of result",
        apple_str(preview)
    );
    let out = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // Cancel raises a non-zero AppleScript error
    }
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    if s.ends_with('\n') {
        s.pop();
    }
    Some(s)
}

/// AppleScript double-quoted string literal. Newlines are flattened to spaces
/// (a command preview is single-line; AppleScript joins with `& return &`).
fn apple_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
