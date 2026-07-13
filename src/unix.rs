//! Shared Unix (Linux + macOS) elevation via `sudo -A`.
//!
//! We use `sudo -A` (askpass) rather than `sudo -S` (password on stdin) so the
//! command's stdin/tty stay free — interactive root commands like `pacman -Syu`
//! ("Proceed? [Y/n]") still work. sudo obtains the password by exec'ing a tiny
//! wrapper script that re-invokes this same binary as `__askpass`, which then
//! shows the platform password dialog and prints the password on stdout. The
//! password never touches argv, our environment, disk, or a log.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Entry point when sudo execs us as the askpass helper.
pub fn askpass_mode() -> ! {
    let preview = std::env::var("VUDO_PREVIEW").unwrap_or_else(|_| "a command".to_string());
    let caller = std::env::var("VUDO_CALLER").unwrap_or_else(|_| "unknown".to_string());
    let interactive = match std::env::var("VUDO_INTERACTIVE").as_deref() {
        Ok("1") => Some(true),
        Ok("0") => Some(false),
        _ => None,
    };
    let cache = std::env::var("VUDO_CACHE").as_deref() == Ok("1");

    #[cfg(target_os = "linux")]
    let pw = crate::linux::ask_password(&preview, &caller, interactive, cache);
    #[cfg(target_os = "macos")]
    let pw = crate::macos::ask_password(&preview, &caller, interactive, cache);

    match pw {
        Some(p) => {
            // sudo reads one line and strips the trailing newline; no newline needed.
            let _ = std::io::stdout().write_all(p.as_bytes());
            std::process::exit(0);
        }
        None => std::process::exit(1), // cancelled -> auth failure
    }
}

pub fn elevate(cmd: &[String], preview: &str, cache: bool) -> i32 {
    // Already root — run it directly.
    if is_root() {
        return run_inherit(&cmd[0], &cmd[1..], &[]);
    }

    if cache {
        // Opt-in (`-c`): reuse sudo's credential window instead of clearing it.
        // If it's still valid, run with no prompt at all; otherwise authorize
        // once below and leave the timestamp intact for the window.
        if sudo_cached() {
            let mut args = vec!["-n".to_string(), "--".to_string()];
            args.extend_from_slice(cmd);
            return run_inherit("sudo", &args, &[]);
        }
    } else {
        // Default: authorize every command on its own. vudo does NOT ride
        // sudo's cached timestamp — otherwise, once you approved one command,
        // later commands in the session would run with no prompt. Clear it so a
        // fresh authorization is always required.
        reset_sudo_timestamp();
    }

    // Who invoked us, and whether they had a controlling terminal — shown in
    // the dialog so the user can see where a root prompt came from (their own
    // terminal vs. an agent/automation).
    let caller = crate::caller::describe();
    let interactive = has_controlling_terminal();
    let interactive_env = if interactive { "1" } else { "0" };

    // On macOS with Touch ID, show the preview once up front (the biometric
    // sheet can't display it), then let pam_tid authorize.
    #[cfg(target_os = "macos")]
    if crate::macos::has_touch_id()
        && !crate::macos::confirm(preview, &caller, Some(interactive), cache)
    {
        eprintln!("vudo: cancelled");
        return 130;
    }

    let wrapper = match AskpassWrapper::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("vudo: could not set up askpass helper: {e}");
            return 1;
        }
    };

    let mut args = vec!["-A".to_string(), "--".to_string()];
    args.extend_from_slice(cmd);
    let code = run_inherit(
        "sudo",
        &args,
        &[
            ("SUDO_ASKPASS", wrapper.path()),
            ("VUDO_PREVIEW", preview),
            ("VUDO_CALLER", caller.as_str()),
            ("VUDO_INTERACTIVE", interactive_env),
            ("VUDO_CACHE", if cache { "1" } else { "0" }),
        ],
    );

    // Default mode: don't leave a cached credential window open afterwards —
    // the next privileged action must re-authorize. In cache mode we keep the
    // timestamp so the window works.
    if !cache {
        reset_sudo_timestamp();
    }
    code
}

fn is_root() -> bool {
    // SAFETY: geteuid is always safe to call.
    unsafe { libc::geteuid() == 0 }
}

/// Whether we have a controlling terminal — a reliable "a human at a keyboard
/// launched this" signal. Opening /dev/tty succeeds only if one exists, and
/// unlike an isatty(stdin) check it isn't fooled by redirected stdio
/// (`vudo id > file` still counts as interactive).
fn has_controlling_terminal() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/tty")
        .is_ok()
}

/// Invalidate any cached sudo credentials so the next `sudo` must re-authorize.
/// `sudo -k` never prompts and needs no privilege; it just clears the timestamp.
///
/// stdin is inherited (not detached) so sudo sees the same controlling terminal
/// as the real `sudo -A` command below. sudo's timestamp is keyed to the tty;
/// detaching stdin would make this clear a *different* record than the one the
/// command uses, leaving the real cache untouched.
fn reset_sudo_timestamp() {
    let _ = Command::new("sudo")
        .arg("-k")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Whether sudo currently has a valid cached credential (used only in `--cache`
/// mode). `sudo -n` never prompts; it succeeds only if no auth is needed. stdin
/// is inherited so it checks the same tty-keyed record the command will use.
fn sudo_cached() -> bool {
    Command::new("sudo")
        .args(["-n", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_inherit(program: &str, args: &[String], env: &[(&str, &str)]) -> i32 {
    let mut c = Command::new(program);
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    match c.status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("vudo: failed to run {program}: {e}");
            127
        }
    }
}

/// A temp `#!/bin/sh` wrapper that re-invokes this binary as `__askpass`.
/// Removed on drop.
struct AskpassWrapper {
    dir: PathBuf,
    file: PathBuf,
}

impl AskpassWrapper {
    fn new() -> std::io::Result<Self> {
        let exe = std::env::current_exe()?;
        let mut dir = std::env::temp_dir();
        dir.push(format!("vudo-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;

        let file = dir.join("askpass");
        let script = format!(
            "#!/bin/sh\nexec {} __askpass \"$@\"\n",
            crate::quote::shell_quote(&exe.to_string_lossy())
        );
        std::fs::write(&file, script)?;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self { dir, file })
    }

    fn path(&self) -> &str {
        self.file.to_str().unwrap_or("")
    }
}

impl Drop for AskpassWrapper {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
