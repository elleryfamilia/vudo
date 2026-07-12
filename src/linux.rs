//! Linux password dialog: zenity → kdialog → pinentry, whichever is present.
//! zenity/kdialog are preferred; pinentry is a last resort (it can be flaky
//! under some compositors, silently returning no dialog).

use std::io::{Read, Write};
use std::process::{Command, Stdio};

pub fn ask_password(preview: &str) -> Option<String> {
    let body = format!(
        "vudo will run this command as root:\n\n{preview}\n\nEnter your password to authorize."
    );

    if have("zenity") {
        return run_capture(
            "zenity",
            &[
                "--entry".to_string(),
                "--hide-text".to_string(),
                "--title=vudo".to_string(),
                format!("--text={body}"),
            ],
        );
    }

    if have("kdialog") {
        return run_capture(
            "kdialog",
            &[
                "--title".to_string(),
                "vudo".to_string(),
                "--password".to_string(),
                body,
            ],
        );
    }

    if let Some(pe) = pinentry_bin() {
        return pinentry_ask(&pe, &body);
    }

    eprintln!("vudo: no graphical password prompt found — install zenity or kdialog");
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
