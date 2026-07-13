//! macOS dialogs via osascript, plus Touch ID detection.

use std::process::{Command, Stdio};

/// Enable Touch ID for sudo by adding `pam_tid` to the PAM config. Idempotent;
/// writes to `/etc/pam.d/sudo_local` (which survives macOS updates) when the
/// system includes it, else to `/etc/pam.d/sudo`. The privileged write goes
/// through the normal vudo dialog so the user sees exactly what runs.
pub fn setup_touch_id() -> i32 {
    if has_touch_id() {
        println!("vudo: Touch ID for sudo is already enabled.");
        return 0;
    }

    // Sonoma+ ships /etc/pam.d/sudo with an `auth include sudo_local` line;
    // there, sudo_local is the update-safe place for the pam_tid entry.
    let uses_sudo_local = std::fs::read_to_string("/etc/pam.d/sudo")
        .map(|s| s.contains("sudo_local"))
        .unwrap_or(false);
    let target = if uses_sudo_local {
        "/etc/pam.d/sudo_local"
    } else {
        "/etc/pam.d/sudo"
    };
    let line = "auth       sufficient     pam_tid.so";

    println!("vudo: this enables Touch ID for sudo by appending to {target}:");
    println!("        {line}\n");

    // Idempotent append, performed as root.
    let script = format!("grep -qs pam_tid.so {target} || printf '%s\\n' '{line}' >> {target}");
    let cmd = vec!["sh".to_string(), "-c".to_string(), script];
    let preview = crate::quote::preview(&cmd);
    let code = crate::unix::elevate(&cmd, &preview, false);

    if code == 0 {
        println!("\nvudo: Touch ID enabled. Keep this terminal open, then in a NEW terminal");
        println!("      run 'sudo -k; sudo -v' — you should get the Touch ID sheet.");
    }
    code
}

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
