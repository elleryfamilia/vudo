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
pub fn confirm(preview: &str, caller: &str, interactive: Option<bool>, cache: bool) -> bool {
    let msg = crate::dialog::info_block(preview, caller, interactive, cache);
    let script = format!(
        "display dialog {} with title \"vudo\" {} \
         buttons {{\"Cancel\", \"Run as root\"}} default button \"Run as root\"",
        apple_text(&msg),
        icon_clause()
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
pub fn ask_password(
    preview: &str,
    caller: &str,
    interactive: Option<bool>,
    cache: bool,
) -> Option<String> {
    let msg = format!(
        "{}\n\nEnter your password to authorize.",
        crate::dialog::info_block(preview, caller, interactive, cache)
    );
    let script = format!(
        "display dialog {} \
         with title \"vudo\" {} \
         default answer \"\" with hidden answer \
         buttons {{\"Cancel\", \"Run as root\"}} default button \"Run as root\"\n\
         return text returned of result",
        apple_text(&msg),
        icon_clause()
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

/// `with icon` clause using the brand icon when available, else the stock
/// caution icon.
fn icon_clause() -> String {
    match crate::icon::path() {
        Some(p) => format!("with icon (POSIX file {})", apple_str(&p)),
        None => "with icon caution".to_string(),
    }
}

/// AppleScript expression for a multi-line string: each line becomes a quoted
/// literal joined with `& return &`, so newlines render as actual line breaks.
fn apple_text(s: &str) -> String {
    s.split('\n')
        .map(apple_str)
        .collect::<Vec<_>>()
        .join(" & return & ")
}

/// AppleScript double-quoted string literal for a single line.
fn apple_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
