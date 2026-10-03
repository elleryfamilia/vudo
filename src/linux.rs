//! Linux password dialog: zenity → kdialog → pinentry, whichever is present.
//! zenity/kdialog are preferred; pinentry is a last resort (it can be flaky
//! under some compositors, silently returning no dialog).
//!
//! Also home to the PAM-stack sniffing that keeps the command preview on
//! systems where sudo can authorize without a typed password (fingerprint
//! readers, security keys): see [`silent_auth_hint`].

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub fn ask_password(
    preview: &str,
    caller: &str,
    interactive: Option<bool>,
    cache: bool,
) -> Option<String> {
    let body = format!(
        "{}\n\nEnter your password to authorize.",
        crate::dialog::info_block(preview, caller, interactive, cache)
    );

    if have("zenity") {
        let mut args = vec![
            "--entry".to_string(),
            "--hide-text".to_string(),
            "--title=vudo".to_string(),
            format!("--text={body}"),
        ];
        // The icon flag each zenity generation natively supports (see
        // zenity_icon_args).
        if let Some(p) = crate::icon::path() {
            args.extend(zenity_icon_args(&p));
        }
        return run_capture("zenity", &args);
    }

    if have("kdialog") {
        let mut args = vec![
            "--title".to_string(),
            "vudo".to_string(),
            "--password".to_string(),
            body,
        ];
        if let Some(p) = crate::icon::path() {
            args.push("--icon".to_string());
            args.push(p);
        }
        return run_capture("kdialog", &args);
    }

    if let Some(pe) = pinentry_bin() {
        return pinentry_ask(&pe, &body);
    }

    eprintln!("vudo: no graphical password prompt found — install zenity, kdialog, or pinentry");
    None
}

/// Preview-only confirmation dialog, shown ahead of sudo's auth when a PAM
/// module can authorize without any typed input (see [`silent_auth_hint`]).
/// In that case sudo never invokes our askpass helper — the dialog that
/// normally carries the command preview — so vudo must show the preview
/// itself or the user would authorize a command they never saw. Returns true
/// when the user chose "Run as root".
pub fn confirm(
    preview: &str,
    caller: &str,
    interactive: Option<bool>,
    cache: bool,
    hint: &str,
) -> bool {
    let body = format!(
        "{}\n\nAfter you confirm, {hint}. If it isn't answered in time, \
         a password prompt appears instead.",
        crate::dialog::info_block(preview, caller, interactive, cache)
    );

    if have("zenity") {
        let mut args = vec![
            "--question".to_string(),
            "--title=vudo".to_string(),
            format!("--text={body}"),
            "--ok-label=Run as root".to_string(),
            "--cancel-label=Cancel".to_string(),
        ];
        // The icon flag each zenity generation natively supports (see
        // zenity_icon_args) — on zenity 3 the wrong one hard-fails at option
        // parsing, which would read as "cancelled" and permanently break
        // elevation on biometric systems.
        if let Some(p) = crate::icon::path() {
            args.extend(zenity_icon_args(&p));
        }
        return Command::new("zenity")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }

    if have("kdialog") {
        // --yes-label/--no-label are kdialog 21.08+; an older kdialog fails
        // hard at option parsing on unknown options, which would read as
        // "cancelled". Ask the binary instead of assuming.
        let mut args = vec!["--title".to_string(), "vudo".to_string()];
        if kdialog_offers_button_labels(&kdialog_help()) {
            args.extend(["--yes-label".to_string(), "Run as root".to_string()]);
            args.extend(["--no-label".to_string(), "Cancel".to_string()]);
        }
        args.push("--yesno".to_string());
        args.push(body);
        if let Some(p) = crate::icon::path() {
            args.push("--icon".to_string());
            args.push(p);
        }
        return Command::new("kdialog")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }

    if let Some(pe) = pinentry_bin() {
        return pinentry_confirm(&pe, &body);
    }

    // No dialog backend at all — the askpass password dialog would fail too,
    // so refuse rather than authorize a command nobody could have previewed.
    eprintln!("vudo: no graphical prompt found — install zenity, kdialog, or pinentry");
    false
}

/// A Touch ID-style notice raised while sudo authenticates: a fingerprint
/// dialog with a pulsing animation, reacting live to the sensor.
///
/// Why it exists: after the confirmation dialog closes, the silent-auth
/// module (pam_fprintd, pam_u2f) starts waiting for a finger — but its PAM
/// prompt ("Place your finger on the fingerprint reader") is printed by sudo
/// to the caller's stdout. In vudo's whole reason for existing there is no
/// terminal to read it, so the user clicks "Run as root" and then faces
/// silence. This dialog fills that window.
///
/// The visuals are the best each backend allows, in order:
/// * **yad** — the fingerprint glyph *inside* the dialog (`--image`),
///   pulsing bar, live text driven by fprintd's D-Bus verify signals.
/// * **zenity** — same bar/text behavior (zenity 4 has no in-dialog image);
///   the glyph rides on the window icon instead.
/// * **kdialog** — a static message box.
/// * **pinentry** — no non-blocking message box exists; skipped.
///
/// Live feedback comes from watching fprintd's system-bus
/// `net.reactivated.Fprint.Device.VerifyStatus` signals (via dbus-monitor,
/// when present): retry conditions update the text ("press flat…"), a
/// no-match asks for another touch, and a match switches to "✓ Authorized"
/// and lets the bar sweep to 100% (--auto-close) — the dialog removes itself
/// the instant auth finishes.
///
/// The notice is advisory only. D-Bus signals on the system bus can be
/// emitted by any local process, so nothing security-relevant is ever wired
/// to it: PAM/fprintd decide authentication, and vudo relays sudo's exit
/// status. Worst case a spoofed signal closes a dialog early or shows a
/// misleading string.
///
/// If the biometric goes unanswered, PAM falls back to a password prompt —
/// sudo invokes the askpass helper, which touches a flag file
/// (`watch_askpass_flag`) and this notice is dismissed immediately, so it
/// never competes with (or contradicts) the password dialog.
pub struct AuthIndicator {
    /// Dialog/monitor processes, shared with the fallback watcher thread;
    /// `take()` under the lock makes dismissal idempotent whichever side
    /// gets there first.
    procs: std::sync::Arc<std::sync::Mutex<Option<Box<IndicatorProcs>>>>,
}

struct IndicatorProcs {
    dialog: Child,
    monitor: Option<Child>,
}

impl IndicatorProcs {
    fn kill_all(&mut self) {
        let _ = self.dialog.kill();
        let _ = self.dialog.wait();
        if let Some(mut monitor) = self.monitor.take() {
            let _ = monitor.kill();
            let _ = monitor.wait();
        }
    }
}

/// Show the notice while sudo authenticates. Never blocks, never fails the
/// authorization: with no dialog backend the auth still runs, just silently
/// (same as before this dialog existed).
pub fn auth_indicator(hint: &str) -> AuthIndicator {
    let text = format!("Now {hint} to authorize.");
    let icon = crate::icon::fingerprint();

    // yad first: it's the only backend that can put the fingerprint glyph
    // inside the dialog. Same progress protocol as zenity: text updates as
    // "# …" lines on stdin, "100" closes with --auto-close.
    let dialog = if have("yad") {
        let mut args = vec![
            "--progress".to_string(),
            "--pulsate".to_string(),
            "--no-cancel".to_string(),
            "--auto-close".to_string(),
            "--title=vudo".to_string(),
            format!("--text={text}"),
        ];
        if let Some(p) = &icon {
            args.push(format!("--image={p}"));
        }
        if let Some(p) = icon.clone().or_else(crate::icon::path) {
            args.push(format!("--window-icon={p}"));
        }
        Command::new("yad")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    } else if have("zenity") {
        let mut args = vec![
            "--progress".to_string(),
            "--pulsate".to_string(),
            "--no-cancel".to_string(),
            "--auto-close".to_string(),
            "--title=vudo".to_string(),
            format!("--text={text}"),
        ];
        if let Some(p) = icon.clone().or_else(crate::icon::path) {
            // The icon flag each zenity generation natively supports (see
            // zenity_icon_args).
            args.extend(zenity_icon_args(&p));
        }
        Command::new("zenity")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    } else if have("kdialog") {
        // No stdin-driven progress updates; at least keep the icon.
        let mut args = vec![
            "--title".to_string(),
            "vudo".to_string(),
            "--msgbox".to_string(),
            text,
        ];
        if let Some(p) = &icon {
            args.push("--icon".to_string());
            args.push(p.clone());
        }
        Command::new("kdialog")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    } else {
        None
    };

    let mut procs = dialog.map(|dialog| {
        Box::new(IndicatorProcs {
            dialog,
            monitor: None,
        })
    });

    // Live sensor feedback needs both a dialog we can update (yad/zenity) and
    // the fprintd signal stream.
    if procs.as_ref().is_some_and(|p| p.dialog.stdin.is_some()) && have("dbus-monitor") {
        if let Ok(mut monitor) = Command::new("dbus-monitor")
            .arg("--system")
            .arg("type='signal',interface='net.reactivated.Fprint.Device',member='VerifyStatus'")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            let feed = procs.as_mut().and_then(|p| p.dialog.stdin.take());
            let stdout = monitor.stdout.take();
            if let (Some(feed), Some(stdout)) = (feed, stdout) {
                std::thread::spawn(move || watch_verify(stdout, feed));
                procs.as_mut().unwrap().monitor = Some(monitor);
            }
        }
    }

    AuthIndicator {
        procs: std::sync::Arc::new(std::sync::Mutex::new(procs)),
    }
}

impl AuthIndicator {
    /// Remove the notice: sudo's auth finished (succeeded or failed), or the
    /// password fallback began ([`Self::watch_askpass_flag`]). Live updates
    /// usually closed the dialog already on a match ("✓ Authorized" +
    /// auto-close); killing covers every other exit. Idempotent.
    pub fn dismiss(&self) {
        let Some(mut procs) = self.procs.lock().ok().and_then(|mut guard| guard.take()) else {
            return;
        };
        procs.kill_all();
    }

    /// Dismiss the notice as soon as the askpass helper runs — i.e. the
    /// moment PAM falls back to asking for a password. The helper touches
    /// `flag` on entry (see `unix::askpass_mode`); this poller watches for
    /// it so the "touch the reader" notice never sits beside the password
    /// dialog telling the user to do the wrong thing. Also exits when the
    /// notice was dismissed by the parent (auth finished some other way).
    pub fn watch_askpass_flag(&self, flag: std::path::PathBuf) {
        let procs = std::sync::Arc::clone(&self.procs);
        std::thread::spawn(move || loop {
            if flag.exists() {
                let Some(mut procs) = procs.lock().ok().and_then(|mut guard| guard.take()) else {
                    return;
                };
                procs.kill_all();
                return;
            }
            // Parent already dismissed (auth finished): stop polling.
            let taken = procs.lock().map(|guard| guard.is_none()).unwrap_or(true);
            if taken {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        });
    }
}

/// Read dbus-monitor's output and translate fprintd's verify results into
/// dialog updates. Ends when the monitor is killed (dismiss) or its stdout
/// closes.
fn watch_verify(stdout: std::process::ChildStdout, mut feed: std::process::ChildStdin) {
    use std::io::{BufRead, BufReader, Write};

    let mut watcher = VerifyWatcher::default();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        match watcher.feed(&line) {
            Some(Feedback::Msg(m)) => {
                let _ = writeln!(feed, "# {m}");
            }
            Some(Feedback::Match) => {
                // A beat of success before the bar sweeps shut.
                let _ = writeln!(feed, "# \u{2713} Authorized");
                std::thread::sleep(std::time::Duration::from_millis(400));
                let _ = writeln!(feed, "100");
                break;
            }
            None => {}
        }
    }
}

/// What fprintd's `VerifyStatus` result maps to on the dialog.
#[derive(Debug, PartialEq)]
enum Feedback {
    /// Text to show (without the leading "# ").
    Msg(&'static str),
    /// The finger was accepted.
    Match,
}

/// Line-by-line state machine over dbus-monitor output. A signal looks like:
///
/// ```text
/// signal time=… path=/net/reactivated/Fprint/Device/0; interface=net.reactivated.Fprint.Device; member=VerifyStatus
///    boolean true
///    string "verify-match"
/// ```
///
/// After a `member=VerifyStatus` header, the next `string` line carries the
/// result; `boolean` (done) is ignored — every result except the final
/// match/no-match arrives with its own signal anyway.
#[derive(Default)]
struct VerifyWatcher {
    in_verify_status: bool,
}

impl VerifyWatcher {
    fn feed(&mut self, line: &str) -> Option<Feedback> {
        if self.in_verify_status {
            if let Some(result) = line.trim().strip_prefix("string \"") {
                self.in_verify_status = false;
                let result = result.trim_end_matches('"');
                return map_result(result);
            }
            // Boolean/space lines inside the signal body: keep waiting for
            // the string, but don't wait forever — a new header resets.
            if !line.starts_with(' ') {
                self.in_verify_status = false;
            }
            return None;
        }

        if line.contains("member=VerifyStatus") {
            self.in_verify_status = true;
        }
        None
    }
}

/// fprintd result names are stable API (libfprint's FpDeviceVerifyResult).
fn map_result(result: &str) -> Option<Feedback> {
    match result {
        "verify-match" => Some(Feedback::Match),
        "verify-no-match" => Some(Feedback::Msg("Not that finger \u{2014} touch again")),
        "verify-swipe-too-short" => Some(Feedback::Msg("Press flat and hold a moment")),
        "verify-finger-not-centered" => Some(Feedback::Msg("Center your finger on the reader")),
        "verify-remove-and-retry" => Some(Feedback::Msg("Lift, then touch again")),
        "verify-retry-scan" | "verify-retry-too-short" | "verify-retry-too-fast" => {
            Some(Feedback::Msg("Try again"))
        }
        // verify-disconnected, verify-unknown-error, … — sudo reports these
        // through its own failure path; the dialog goes away regardless.
        _ => None,
    }
}

const PAM_DIR: &str = "/etc/pam.d";

/// PAM modules that can complete authorization without any typed input, with
/// a short user-facing hint for each. When one of these succeeds first, sudo
/// never asks for a password, which means sudo -A never invokes our askpass
/// helper — and the command preview it carries is never shown.
const SILENT_AUTH_MODULES: &[(&str, &str)] = &[
    ("pam_fprintd.so", "touch the fingerprint reader"),
    ("pam_u2f.so", "tap your security key"),
];

/// Some(`"touch the fingerprint reader"`) if sudo's effective PAM stack can
/// authorize on its own — an *uncommented* `auth` line for one of
/// [`SILENT_AUTH_MODULES`] whose control flag lets it complete the stack
/// (`sufficient`, or a bracketed flag with a `done` success action). The
/// returned hint tells the user what to do after confirming. Includes are
/// followed (`auth include system-auth` on Arch-style layouts, `@include
/// common-auth` on Debian-style ones) since the module is rarely in the
/// sudo file itself.
///
/// Ordering is deliberately not evaluated: a `sufficient` biometric line
/// placed *after* a required password module still triggers the confirm
/// dialog even though sudo asks for the password first. That's benign —
/// the password dialog carries its own preview, and the notice is dismissed
/// the moment the askpass helper runs.
pub fn silent_auth_hint() -> Option<&'static str> {
    let mut visited = Vec::new();
    pam_allows_silent_auth(Path::new(PAM_DIR), "sudo", &mut visited)
}

/// Depth-first walk of a PAM config, following includes. Every visited file
/// is expanded at most once, which also guards against include cycles.
fn pam_allows_silent_auth(
    dir: &Path,
    file: &str,
    visited: &mut Vec<String>,
) -> Option<&'static str> {
    if visited.iter().any(|f| f == file) {
        return None; // include cycle — Debian's common-* files can form these
    }
    visited.push(file.to_string());

    let contents = std::fs::read_to_string(dir.join(file)).ok()?;
    for line in contents.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        match fields.next() {
            // Debian-style include: `@include common-auth`
            Some("@include") => {
                if let Some(f) = fields.next() {
                    if let Some(h) = pam_allows_silent_auth(dir, f, visited) {
                        return Some(h);
                    }
                }
            }
            Some("auth") => {
                let rest: Vec<&str> = fields.collect();
                if let Some(h) = pam_auth_line(&rest, dir, visited) {
                    return Some(h);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether one `auth`-type line (fields after the type) enables silent auth.
fn pam_auth_line(rest: &[&str], dir: &Path, visited: &mut Vec<String>) -> Option<&'static str> {
    // The control field is a bare word (`sufficient`, `include`, ...) or a
    // bracketed flag — which contains spaces, so it spans several
    // whitespace-separated fields: "[success=done default=ignore]". The
    // module name only starts after it.
    let (control, module) = if rest.first()?.starts_with('[') {
        let k = rest.iter().position(|t| t.ends_with(']'))?; // malformed flag ends with no ']'

        (rest[..=k].join(" "), rest.get(k + 1).copied())
    } else {
        (rest[0].to_string(), rest.get(1).copied())
    };

    // `include`/`substack` splice another file's auth lines into the stack.
    if control == "include" || control == "substack" {
        return module.and_then(|f| pam_allows_silent_auth(dir, f, visited));
    }

    // Only a module that can complete the stack on its own can skip the
    // askpass dialog. With `required`/`requisite`/`optional` a password is
    // still needed later, so the preview stays where it belongs.
    // Known limitation: a `success=N` jump control (`[success=1
    // default=ignore]`, the passwordless-with-fallback idiom) is *not*
    // detected — knowing where the jump lands would need full stack
    // evaluation, and those setups keep the pre-existing behavior (no extra
    // confirm dialog). Also, `done` must be the *success* action:
    // `[auth_err=done]` wouldn't authorize alone, so it must not count.
    let finishes_stack =
        control == "sufficient" || (control.starts_with('[') && control.contains("success=done"));
    if finishes_stack {
        if let Some(m) = module {
            if let Some((_, hint)) = SILENT_AUTH_MODULES
                .iter()
                .find(|(name, _)| m.contains(name))
            {
                return Some(hint);
            }
        }
    }
    None
}

fn run_capture(program: &str, args: &[String]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // cancelled
    }
    Some(trim_newline(
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

fn trim_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

fn have(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether the installed zenity accepts `--icon` — zenity 4+. Decided once
/// per process; any doubt (older zenity, unparseable `--version` output,
/// spawn failure) falls back to `--window-icon`, which works on both
/// generations: zenity 3 hard-fails unknown options where zenity 4 only
/// deprecation-warns, and the warning lands on the user's stderr in the
/// password dialog — so prefer each generation's native option.
fn zenity_accepts_icon_flag() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        Command::new("zenity")
            .arg("--version")
            .output()
            .ok()
            .map(|o| zenity_version_supports_icon(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or(false)
    })
}

/// Major version >= 4. Anything unparseable reads as "assume old", the option
/// that's safe everywhere.
fn zenity_version_supports_icon(version: &str) -> bool {
    version
        .trim()
        .split('.')
        .next()
        .and_then(|major| major.trim().parse::<u32>().ok())
        .is_some_and(|major| major >= 4)
}

/// The icon flag the installed zenity natively supports.
fn zenity_icon_args(icon: &str) -> Vec<String> {
    std::iter::once(if zenity_accepts_icon_flag() {
        format!("--icon={icon}")
    } else {
        format!("--window-icon={icon}")
    })
    .collect()
}

/// `kdialog --help` output, empty on any failure.
fn kdialog_help() -> String {
    Command::new("kdialog")
        .arg("--help")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Whether a `kdialog --help` text advertises custom button labels.
fn kdialog_offers_button_labels(help: &str) -> bool {
    help.contains("--yes-label")
}

/// Pick the first pinentry that actually runs — a broken install (e.g. missing
/// Qt libs) can sit on PATH but fail on launch. The handshake shows no dialog.
fn pinentry_bin() -> Option<String> {
    for p in ["pinentry-gnome3", "pinentry-qt", "pinentry"] {
        if !have(p) {
            continue;
        }
        let mut child = match Command::new(p)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Some(si) = child.stdin.as_mut() {
            let _ = si.write_all(b"GETINFO version\nBYE\n");
        }
        if let Ok(status) = child.wait() {
            if status.success() {
                return Some(p.to_string());
            }
        }
    }
    None
}

fn pinentry_ask(bin: &str, body: &str) -> Option<String> {
    let script = format!(
        "SETTITLE {}\nSETDESC {}\nSETPROMPT {}\nGETPIN\nBYE\n",
        assuan_esc("vudo (sudo)"),
        assuan_esc(body),
        assuan_esc("Password:"),
    );

    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // The script is small enough to fit the pipe buffer, so writing it all and
    // closing stdin won't deadlock: pinentry reads the commands, blocks on
    // GETPIN's dialog, then we read its full response.
    child.stdin.take()?.write_all(script.as_bytes()).ok()?;

    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let _ = child.wait();

    parse_pinentry(&out)
}

/// Preview-only confirmation via pinentry's CONFIRM command. pinentry has no
/// keyhole UI here, so the description rides in CONFIRM's own text argument
/// rather than SETDESC — that way a pinentry that doesn't implement SETDESC
/// still shows the preview. Confirmed iff a second OK follows the greeting.
fn pinentry_confirm(bin: &str, body: &str) -> bool {
    let script = format!("CONFIRM {}\nBYE\n", assuan_esc(body));

    let mut child = match Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    // Same pipe-buffer reasoning as pinentry_ask: the script always fits.
    if child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .is_err()
    {
        return false;
    }

    let mut out = String::new();
    if child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .is_err()
    {
        return false;
    }
    let _ = child.wait();

    parse_pinentry_confirm(&out)
}

/// Confirmed iff the response is unambiguous: at least two OK lines (the
/// Assuan greeting plus CONFIRM's own OK — BYE's trailing "OK closing
/// connection" makes three) and no ERR line anywhere. Any ERR — the user
/// declined, the backend doesn't implement CONFIRM, or the transport broke —
/// means not confirmed: fail closed, since the alternative is treating a
/// refusal as authorization. (A long preview can also overflow libassuan's
/// line limit and come back as ERR; declining there too is the safe side.)
fn parse_pinentry_confirm(out: &str) -> bool {
    out.lines().filter(|l| l.starts_with("OK")).count() >= 2
        && !out.lines().any(|l| l.starts_with("ERR"))
}

/// Reassemble the PIN from an Assuan response. A long PIN can arrive across
/// several "D " continuation lines; concatenate their payloads, then undo the
/// percent-escaping pinentry applies to '%', CR, and LF.
fn parse_pinentry(stdout: &str) -> Option<String> {
    let mut enc = String::new();
    let mut saw_data = false;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("D ") {
            enc.push_str(rest);
            saw_data = true;
        }
    }
    if !saw_data {
        return None; // cancelled -> only OK/ERR, no D line
    }
    Some(assuan_decode(&enc))
}

fn assuan_esc(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn assuan_decode(s: &str) -> String {
    s.replace("%0A", "\n")
        .replace("%0a", "\n")
        .replace("%0D", "\r")
        .replace("%0d", "\r")
        .replace("%25", "%")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn kdialog_help_probe_matches_real_usage_text() {
        // Modern kdialog (21.08+) advertises --yes-label in --help.
        assert!(kdialog_offers_button_labels(
            "Usage: kdialog [options]\n--yes-label <text>\n--yesno <text>\n"
        ));
        // Older kdialog: plain yes/no only — must not count.
        assert!(!kdialog_offers_button_labels(
            "Usage: kdialog [options]\n--yesno <text>\n--msgbox <text>\n"
        ));
    }

    #[test]
    fn zenity_version_probe_classifies_both_generations() {
        assert!(zenity_version_supports_icon("4.2.2\n"));
        assert!(zenity_version_supports_icon("4\n"));
        assert!(!zenity_version_supports_icon("3.44.3\n"));
        assert!(
            !zenity_version_supports_icon("zenity 3.44\n"),
            "prefixed/unparseable → assume old"
        );
        assert!(!zenity_version_supports_icon(""), "no output → assume old");
        assert!(!zenity_version_supports_icon("garbage\n"));
    }

    // PAM-stack sniffing: scratch /etc/pam.d trees exercising the layouts we
    // claim to understand, run through the same code path the real check uses
    // (only the directory differs).

    fn scratch_pam(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vudo-pam-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (f, contents) in files {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(f), contents).unwrap();
        }
        dir
    }

    fn sniff(dir: &Path) -> Option<&'static str> {
        let mut visited = Vec::new();
        pam_allows_silent_auth(dir, "sudo", &mut visited)
    }

    const ARCH_SUDO: &str = "\
#%PAM-1.0
auth\t\tinclude\t\tsystem-auth
account\t\tinclude\t\tsystem-auth
session\t\tinclude\t\tsystem-auth
";

    #[test]
    fn arch_include_with_sufficient_fprintd_is_detected() {
        let dir = scratch_pam(
            "arch",
            &[
                ("sudo", ARCH_SUDO),
                ("system-auth", "auth       required   pam_faillock.so\nauth       sufficient  pam_fprintd.so  try_first_pass\nauth       required   pam_unix.so\n"),
            ],
        );
        assert_eq!(sniff(&dir), Some("touch the fingerprint reader"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debian_at_include_is_followed() {
        let dir = scratch_pam(
            "debian",
            &[
                ("sudo", "# pam sudo\n@include common-auth\n"),
                (
                    "common-auth",
                    "auth sufficient pam_fprintd.so\nauth required pam_unix.so\n",
                ),
            ],
        );
        assert_eq!(sniff(&dir), Some("touch the fingerprint reader"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_include_file_is_not_silent_auth() {
        let dir = scratch_pam("missing", &[("sudo", ARCH_SUDO)]);
        assert_eq!(sniff(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commented_module_does_not_count() {
        let dir = scratch_pam(
            "commented",
            &[(
                "sudo",
                "#auth sufficient pam_fprintd.so\nauth required pam_unix.so\n",
            )],
        );
        assert_eq!(sniff(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn required_fprintd_still_needs_a_password() {
        // A required module can't complete the stack on its own, so the
        // askpass dialog (and its preview) still shows — nothing to fix.
        let dir = scratch_pam(
            "required",
            &[(
                "sudo",
                "auth required pam_fprintd.so\nauth required pam_unix.so\n",
            )],
        );
        assert_eq!(sniff(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bracketed_done_control_flag_counts() {
        let dir = scratch_pam(
            "bracketed",
            &[(
                "sudo",
                "auth [success=done default=ignore] pam_fprintd.so\nauth required pam_unix.so\n",
            )],
        );
        assert_eq!(sniff(&dir), Some("touch the fingerprint reader"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bracketed_non_done_control_flag_does_not_count() {
        let dir = scratch_pam(
            "bracketed-soft",
            &[(
                "sudo",
                "auth [success=ignore default=bad] pam_fprintd.so\nauth required pam_unix.so\n",
            )],
        );
        assert_eq!(sniff(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn done_on_a_failure_action_does_not_count() {
        // `done` as a *failure* action (auth_err=done) can't authorize alone,
        // so it must not trigger the confirm dialog.
        let dir = scratch_pam(
            "bracketed-err-done",
            &[(
                "sudo",
                "auth [success=ignore auth_err=done] pam_fprintd.so\nauth required pam_unix.so\n",
            )],
        );
        assert_eq!(sniff(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn substack_include_is_followed() {
        let dir = scratch_pam(
            "substack",
            &[
                ("sudo", "auth substack common-auth\n"),
                (
                    "common-auth",
                    "auth sufficient pam_u2f.so\nauth required pam_unix.so\n",
                ),
            ],
        );
        assert_eq!(sniff(&dir), Some("tap your security key"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn include_cycles_terminate() {
        let dir = scratch_pam(
            "cycle",
            &[
                ("sudo", "auth include a\n"),
                ("a", "auth include b\n"),
                ("b", "auth include a\n"),
            ],
        );
        assert_eq!(sniff(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_auth_lines_are_ignored() {
        let dir = scratch_pam(
            "non-auth",
            &[
                ("sudo", ARCH_SUDO),
                (
                    "system-auth",
                    "session required pam_fprintd.so\nauth required pam_unix.so\n",
                ),
            ],
        );
        assert_eq!(sniff(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absolute_module_path_is_matched() {
        // Distros differ on whether the module is named or given in full.
        let dir = scratch_pam(
            "abs-path",
            &[(
                "sudo",
                "auth sufficient /usr/lib64/security/pam_fprintd.so\nauth required pam_unix.so\n",
            )],
        );
        assert_eq!(sniff(&dir), Some("touch the fingerprint reader"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pinentry_confirm_parses_ok_and_err() {
        // Real transcripts: every session ends with BYE, and libassuan always
        // acknowledges it with a final "OK closing connection" — including a
        // declined CONFIRM, which must NOT parse as confirmed.
        assert!(parse_pinentry_confirm(
            "OK Pleased to meet you\nOK\nOK closing connection\n"
        ));
        assert!(!parse_pinentry_confirm(
            "OK Pleased to meet you\nERR 83886179 not confirmed\nOK closing connection\n"
        ));
        // Backend doesn't implement CONFIRM, or the transport broke mid-way:
        // fail closed.
        assert!(!parse_pinentry_confirm(
            "OK Pleased to meet you\nERR 83886179 unknown IPC command\n"
        ));
        assert!(!parse_pinentry_confirm("OK Pleased to meet you\n"));
    }

    // fprintd signal parsing: samples of real dbus-monitor output.

    const SIGNAL_HEADER: &str = "signal time=1770000000.000000 sender=:1.42 -> destination=(null) serial=7 path=/net/reactivated/Fprint/Device/0; interface=net.reactivated.Fprint.Device; member=VerifyStatus";

    #[test]
    fn verify_match_maps_to_match_feedback() {
        let mut w = VerifyWatcher::default();
        assert_eq!(w.feed(SIGNAL_HEADER), None);
        assert_eq!(w.feed("   boolean true"), None);
        assert!(matches!(
            w.feed("   string \"verify-match\""),
            Some(Feedback::Match)
        ));
    }

    #[test]
    fn verify_no_match_maps_to_try_again_message() {
        let mut w = VerifyWatcher::default();
        w.feed(SIGNAL_HEADER);
        let f = w.feed("   string \"verify-no-match\"");
        assert!(matches!(
            f,
            Some(Feedback::Msg("Not that finger \u{2014} touch again"))
        ));
    }

    #[test]
    fn retry_conditions_map_to_coaching_text() {
        for (result, want) in [
            ("verify-swipe-too-short", "Press flat and hold a moment"),
            (
                "verify-finger-not-centered",
                "Center your finger on the reader",
            ),
            ("verify-retry-scan", "Try again"),
        ] {
            let mut w = VerifyWatcher::default();
            w.feed(SIGNAL_HEADER);
            match w.feed(&format!("   string \"{result}\"")) {
                Some(Feedback::Msg(m)) => assert_eq!(m, want, "for {result}"),
                other => panic!("{result} should map to a message, got {other:?}"),
            }
        }
    }

    #[test]
    fn unrelated_signals_are_ignored() {
        let mut w = VerifyWatcher::default();
        // A different member must not arm the watcher…
        let other = SIGNAL_HEADER.replace("VerifyStatus", "VerifyFingerSelected");
        assert_eq!(w.feed(&other), None);
        assert_eq!(w.feed("   string \"right-index-finger\""), None);
        // …and neither must stray strings outside a signal body.
        assert_eq!(w.feed("string \"verify-match\""), None);
        assert_eq!(w.feed(SIGNAL_HEADER), None);
        assert!(matches!(
            w.feed("   string \"verify-match\""),
            Some(Feedback::Match)
        ));
    }

    #[test]
    fn malformed_signal_body_does_not_hang_the_state() {
        // Header followed by a non-indented line (no string arg): the
        // watcher must reset instead of staying armed forever.
        let mut w = VerifyWatcher::default();
        w.feed(SIGNAL_HEADER);
        assert_eq!(w.feed("signal time=1.0 member=SomethingElse"), None);
        // Still armed? A later bare string line could then be mistaken for a
        // result. feed() resets on non-indented lines, so it isn't:
        assert_eq!(w.feed("   string \"verify-match\""), None);
    }

    #[test]
    fn every_fprintd_result_is_handled_or_deliberately_ignored() {
        // The full libfprint verify result set. Known-coaching results map
        // to feedback; terminal errors map to None — sudo reports those
        // through its own failure path, so the dialog shouldn't guess.
        for r in [
            "verify-match",
            "verify-no-match",
            "verify-swipe-too-short",
            "verify-finger-not-centered",
            "verify-remove-and-retry",
            "verify-retry-scan",
            "verify-retry-too-short",
            "verify-retry-too-fast",
        ] {
            let mut w = VerifyWatcher::default();
            w.feed(SIGNAL_HEADER);
            assert!(
                w.feed(&format!("   string \"{r}\"")).is_some(),
                "{r} should map to feedback"
            );
        }
        for r in ["verify-disconnected", "verify-unknown-error"] {
            let mut w = VerifyWatcher::default();
            w.feed(SIGNAL_HEADER);
            assert_eq!(
                w.feed(&format!("   string \"{r}\"")),
                None,
                "{r} should be ignored"
            );
        }
    }

    #[test]
    fn single_line_pin_with_space_preserved() {
        assert_eq!(
            parse_pinentry("OK\nD Test%2512 xy\nOK\n").as_deref(),
            Some("Test%12 xy")
        );
    }

    #[test]
    fn continuation_lines_concatenated() {
        assert_eq!(
            parse_pinentry("OK\nD Test%2512\nD  xy\nOK\n").as_deref(),
            Some("Test%12 xy")
        );
    }

    #[test]
    fn leading_space_preserved() {
        assert_eq!(
            parse_pinentry("OK\nD  hunter2\nOK\n").as_deref(),
            Some(" hunter2")
        );
    }

    #[test]
    fn encoded_newline_and_cr_decode() {
        assert_eq!(
            parse_pinentry("OK\nD a%0Ab%0Dc\nOK\n").as_deref(),
            Some("a\nb\rc")
        );
    }

    #[test]
    fn literal_percent_is_not_a_false_escape() {
        // user typed "%0A" as three chars -> pinentry sends %250A
        assert_eq!(parse_pinentry("OK\nD %250A\nOK\n").as_deref(), Some("%0A"));
    }

    #[test]
    fn cancel_returns_none() {
        assert_eq!(parse_pinentry("OK\nERR 83886179 cancelled\n"), None);
    }

    #[test]
    fn esc_decode_round_trips() {
        for v in [
            "p@ss w0rd",
            "100%sure",
            "a\nb",
            "trailing ",
            "  ",
            "quote'd",
        ] {
            assert_eq!(assuan_decode(&assuan_esc(v)), v);
        }
    }
}
