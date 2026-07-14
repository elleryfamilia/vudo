//! macOS dialogs via osascript, plus Touch ID detection.

use std::process::{Command, Stdio};

const TID_LINE: &str = "auth       sufficient     pam_tid.so";

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
    // there, sudo_local is the update-safe place for the pam_tid entry. Without
    // that include, sudo_local is inert and the entry must go in sudo itself.
    let sudo_conf = std::fs::read_to_string("/etc/pam.d/sudo").unwrap_or_default();
    let uses_sudo_local = pam_auth_line(&sudo_conf, "sudo_local");
    let target = if uses_sudo_local {
        "/etc/pam.d/sudo_local"
    } else {
        "/etc/pam.d/sudo"
    };

    println!("vudo: this enables Touch ID for sudo by adding to {target}:");
    println!("        {TID_LINE}\n");

    let script = tid_script(target, uses_sudo_local);
    let cmd = vec!["sh".to_string(), "-c".to_string(), script];
    let preview = crate::quote::preview(&cmd);
    let code = crate::unix::elevate(&cmd, &preview, false);
    if code != 0 {
        return code;
    }

    // Don't trust exit 0 — confirm the line is actually active before
    // declaring victory.
    if !has_touch_id() {
        eprintln!("vudo: setup ran, but {target} still has no active pam_tid line;");
        eprintln!("      please inspect the file and report a bug.");
        return 1;
    }

    println!("\nvudo: Touch ID enabled. Keep this terminal open, then in a NEW terminal");
    println!("      run 'sudo -k; sudo -v' — you should get the Touch ID sheet.");
    println!("      No sheet? tmux/screen, SSH, a closed MacBook lid, and iTerm2's");
    println!("      \"sessions survive logging out\" setting all prevent it from showing.");
    0
}

/// Idempotent shell one-liner (run as root) that enables `pam_tid` in
/// `target`. The grep matches only *uncommented* pam_tid lines: Apple's
/// sudo_local.template ships the line commented out, and a template-copied
/// file must not count as already enabled.
fn tid_script(target: &str, append: bool) -> String {
    let t = crate::quote::shell_quote(target);
    if append {
        // Dedicated include file (sudo_local): position doesn't matter.
        format!("grep -qs '^[^#]*pam_tid\\.so' {t} || printf '%s\\n' '{TID_LINE}' >> {t}")
    } else {
        // Editing /etc/pam.d/sudo itself: pam_tid must run before the
        // password modules, so insert it as the first line.
        format!("grep -qs '^[^#]*pam_tid\\.so' {t} || sed -i '' '1i\\\n{TID_LINE}\n' {t}")
    }
}

/// True if `pam_tid` (Touch ID for sudo) is active: an uncommented auth line
/// in /etc/pam.d/sudo, or in /etc/pam.d/sudo_local when (and only when) sudo's
/// config actually includes it — a stray sudo_local on an older system is
/// ignored by PAM and must not count.
pub fn has_touch_id() -> bool {
    let sudo_conf = std::fs::read_to_string("/etc/pam.d/sudo").unwrap_or_default();
    if pam_auth_line(&sudo_conf, "pam_tid.so") {
        return true;
    }
    pam_auth_line(&sudo_conf, "sudo_local")
        && std::fs::read_to_string("/etc/pam.d/sudo_local")
            .map(|s| pam_auth_line(&s, "pam_tid.so"))
            .unwrap_or(false)
}

/// Whether a PAM config has an uncommented `auth` line mentioning `needle`.
fn pam_auth_line(contents: &str, needle: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with('#') && line.starts_with("auth") && line.contains(needle)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = "# sudo_local: local config file which survives system update\n\
         # uncomment following line to enable Touch ID for sudo\n\
         #auth       sufficient     pam_tid.so\n";
    const VENTURA_SUDO: &str = "# sudo: auth account password session\n\
         auth       sufficient     pam_smartcard.so\n\
         auth       required       pam_opendirectory.so\n\
         account    required       pam_permit.so\n";
    const SONOMA_SUDO: &str = "# sudo: auth account password session\n\
         auth       include        sudo_local\n\
         auth       sufficient     pam_smartcard.so\n\
         auth       required       pam_opendirectory.so\n";

    #[test]
    fn commented_pam_lines_do_not_count() {
        assert!(!pam_auth_line(TEMPLATE, "pam_tid.so"));
        assert!(pam_auth_line(
            &format!("{TEMPLATE}{TID_LINE}\n"),
            "pam_tid.so"
        ));
    }

    #[test]
    fn sudo_local_include_detection() {
        assert!(pam_auth_line(SONOMA_SUDO, "sudo_local"));
        assert!(!pam_auth_line(VENTURA_SUDO, "sudo_local"));
        // a commented-out include doesn't count either
        assert!(!pam_auth_line("#auth include sudo_local\n", "sudo_local"));
    }

    // The rest exercise the generated shell against scratch files, with the
    // same /bin/sh, grep, and BSD sed the real setup uses.

    fn run(script: &str) -> bool {
        std::process::Command::new("sh")
            .args(["-c", script])
            .status()
            .unwrap()
            .success()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vudo-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn append_creates_file_and_is_idempotent() {
        let f = scratch("sudo_local-fresh");
        let _ = std::fs::remove_file(&f);
        let script = tid_script(f.to_str().unwrap(), true);

        assert!(run(&script));
        let once = std::fs::read_to_string(&f).unwrap();
        assert!(pam_auth_line(&once, "pam_tid.so"));

        assert!(run(&script), "second run must be a no-op, not a failure");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), once);
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn commented_template_line_does_not_block_the_append() {
        let f = scratch("sudo_local-template");
        std::fs::write(&f, TEMPLATE).unwrap();
        let script = tid_script(f.to_str().unwrap(), true);

        assert!(run(&script));
        assert!(pam_auth_line(
            &std::fs::read_to_string(&f).unwrap(),
            "pam_tid.so"
        ));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn insert_puts_pam_tid_before_the_password_modules() {
        let f = scratch("sudo-ventura");
        std::fs::write(&f, VENTURA_SUDO).unwrap();
        let script = tid_script(f.to_str().unwrap(), false);

        assert!(run(&script));
        let once = std::fs::read_to_string(&f).unwrap();
        let tid = once.lines().position(|l| l.contains("pam_tid.so")).unwrap();
        let pw = once
            .lines()
            .position(|l| l.contains("pam_opendirectory.so"))
            .unwrap();
        assert!(tid < pw, "pam_tid must come before pam_opendirectory");
        assert!(once.ends_with('\n'));

        assert!(run(&script), "second run must be a no-op, not a failure");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), once);
        let _ = std::fs::remove_file(&f);
    }
}
