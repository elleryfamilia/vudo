//! Best-effort identification of what invoked vudo, shown in the auth dialog
//! so the user can see where a root prompt originated — their own terminal vs.
//! an agent/automation (e.g. an AI coding CLI).
//!
//! We show a single, meaningful name: the nearest ancestor that isn't a shell
//! or a pass-through wrapper (`timeout`, `env`, `nohup`, `sudo`, …). Those are
//! noise — `vudo` is almost always spawned through one — so the interesting
//! actor is the first "real" process above them: `claude` rather than `zsh` or
//! `timeout`, or the terminal emulator when you run it yourself.
//!
//! Computed in the MAIN vudo process: its parent is the real caller. (The
//! askpass helper is a separate sudo-spawned child, so it receives the result
//! via env rather than recomputing it.)

/// From a parent-first chain of process names, pick the nearest one that isn't
/// a shell or pass-through wrapper; fall back to the immediate parent, then
/// "unknown".
pub fn pick(chain: &[String]) -> String {
    for name in chain {
        if !is_passthrough(name) {
            return name.clone();
        }
    }
    chain
        .first()
        .cloned()
        .unwrap_or_else(|| "unknown".to_string())
}

/// Shells and thin wrappers that merely relay to another program — not the
/// actor we want to attribute the invocation to.
fn is_passthrough(name: &str) -> bool {
    let lower = name.trim_start_matches('-').to_ascii_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(lower.as_str());
    matches!(
        base,
        // shells
        "sh" | "bash" | "zsh" | "dash" | "fish" | "ksh" | "tcsh" | "csh" | "ash"
            | "pwsh" | "powershell" | "cmd"
        // pass-through wrappers / launchers
            | "timeout" | "env" | "nohup" | "nice" | "ionice" | "stdbuf"
            | "setsid" | "time" | "sudo" | "doas" | "su" | "watch"
            | "script" | "unbuffer" | "xargs"
    )
}

#[cfg(unix)]
pub fn describe() -> String {
    let mut chain = Vec::new();
    // SAFETY: getppid is always safe to call.
    let mut pid = unsafe { libc::getppid() };
    let mut depth = 0;
    while pid > 1 && depth < 8 {
        match proc_name(pid) {
            Some(name) => chain.push(name),
            None => break,
        }
        match parent_pid(pid) {
            Some(ppid) => pid = ppid,
            None => break,
        }
        depth += 1;
    }
    pick(&chain)
}

#[cfg(target_os = "linux")]
fn proc_name(pid: i32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(target_os = "linux")]
fn parent_pid(pid: i32) -> Option<i32> {
    // /proc/<pid>/stat: "pid (comm) state ppid ...". comm can contain spaces
    // and parens, so scan past the LAST ')' before splitting fields.
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = &s[s.rfind(')')? + 1..];
    let mut fields = after.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(target_os = "macos")]
fn proc_name(pid: i32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // comm may be a full path; show just the basename.
    Some(t.rsplit('/').next().unwrap_or(t).to_string())
}

#[cfg(target_os = "macos")]
fn parent_pid(pid: i32) -> Option<i32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

// Other Unix (BSD, etc.): no lookup wired up yet.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn proc_name(_pid: i32) -> Option<String> {
    None
}
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn parent_pid(_pid: i32) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::pick;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn picks_first_non_shell() {
        assert_eq!(pick(&v(&["zsh", "claude", "zsh", "cosmic-term"])), "claude");
    }

    #[test]
    fn skips_wrappers_like_timeout_and_env() {
        assert_eq!(pick(&v(&["timeout", "zsh", "claude"])), "claude");
        assert_eq!(pick(&v(&["env", "nohup", "myapp"])), "myapp");
    }

    #[test]
    fn manual_run_shows_terminal() {
        assert_eq!(pick(&v(&["zsh", "cosmic-term"])), "cosmic-term");
    }

    #[test]
    fn login_shell_dash_prefix_is_a_shell() {
        assert_eq!(pick(&v(&["-zsh", "sshd"])), "sshd");
    }

    #[test]
    fn windows_exe_shells_skipped() {
        assert_eq!(pick(&v(&["powershell.exe", "node.exe"])), "node.exe");
    }

    #[test]
    fn all_shells_falls_back_to_immediate_parent() {
        assert_eq!(pick(&v(&["bash", "zsh"])), "bash");
    }

    #[test]
    fn empty_is_unknown() {
        assert_eq!(pick(&[]), "unknown");
    }
}
