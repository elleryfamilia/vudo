//! Best-effort identification of the process chain that invoked vudo, shown in
//! the auth dialog so the user can see where a root prompt originated — their
//! own shell at a terminal vs. an agent/automation (e.g. an AI coding CLI).
//!
//! Computed in the MAIN vudo process: its parent is the real caller. (The
//! askpass helper is a separate sudo-spawned child, so it must receive this
//! string via env rather than recompute it.)

/// A short "parent ← grandparent ← …" chain of process names, or "unknown".
pub fn describe() -> String {
    let mut names = Vec::new();
    // SAFETY: getppid is always safe to call.
    let mut pid = unsafe { libc::getppid() };
    let mut depth = 0;
    while pid > 1 && depth < 5 {
        match proc_name(pid) {
            Some(name) => names.push(name),
            None => break,
        }
        match parent_pid(pid) {
            Some(ppid) => pid = ppid,
            None => break,
        }
        depth += 1;
    }
    if names.is_empty() {
        "unknown".to_string()
    } else {
        names.join(" \u{2190} ") // " ← "
    }
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
