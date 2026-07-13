//! vudo — run a command as root with a graphical prompt that previews the
//! exact command before you authorize it.
//!
//! Cross-platform by delegating authentication to each OS's native agent
//! rather than drawing our own password field:
//!   * Linux — `sudo -A` with an askpass helper backed by zenity / kdialog /
//!     pinentry. Keeps the command's stdin/tty free so interactive root
//!     commands (e.g. `pacman -Syu`) still work.
//!   * macOS — `sudo` via Touch ID (`pam_tid`) when configured, else an
//!     osascript password dialog.
//!   * Windows — UAC: PowerShell `Start-Process -Verb RunAs`, with a preview
//!     shown first as a message box.

mod quote;

mod caller;
mod dialog;
#[cfg(unix)]
mod icon;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
mod update;
#[cfg(windows)]
mod win;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // On Unix, sudo re-invokes us as the askpass helper.
    #[cfg(unix)]
    if args.first().map(String::as_str) == Some("__askpass") {
        unix::askpass_mode(); // diverges
    }

    match classify(args) {
        Action::Empty => {
            usage();
            std::process::exit(2);
        }
        Action::Help => {
            usage();
            std::process::exit(0);
        }
        Action::Version => {
            println!("vudo {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        Action::Update => std::process::exit(update::run()),
        Action::SetupTouchId => std::process::exit(setup_touch_id()),
        Action::UnknownOption(opt) => {
            // A real command never starts with '-', so an unrecognized leading
            // option is a mistake — reject it instead of trying to run it as
            // root (which would pop a password prompt for a bogus "command").
            eprintln!("vudo: unknown option '{opt}' (try 'vudo --help')");
            std::process::exit(2);
        }
        Action::Run { cmd, cache } => {
            let preview = quote::preview(&cmd);
            std::process::exit(elevate(&cmd, &preview, cache));
        }
    }
}

enum Action {
    Empty,
    Help,
    Version,
    Update,
    SetupTouchId,
    UnknownOption(String),
    Run { cmd: Vec<String>, cache: bool },
}

/// Decide what a vudo invocation means. `--help`/`--version`/`--update` are
/// recognized only as the leading token. Otherwise leading modifier flags
/// (`-c`/`--cache`) are consumed, then everything else is the command to run as
/// root. A bare `--` ends option parsing, so a command whose name starts with
/// `-` can still be run (`vudo -- --weird-tool`).
fn classify(args: Vec<String>) -> Action {
    match args.first().map(String::as_str) {
        None => return Action::Empty,
        Some("-h") | Some("--help") => return Action::Help,
        Some("-V") | Some("--version") => return Action::Version,
        Some("--update") => return Action::Update,
        Some("--setup-touch-id") => return Action::SetupTouchId,
        _ => {}
    }

    let mut cache = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--cache" => {
                cache = true;
                i += 1;
            }
            "--" => {
                i += 1;
                break;
            }
            opt if opt.starts_with('-') => return Action::UnknownOption(opt.to_string()),
            _ => break,
        }
    }

    let cmd = args[i..].to_vec();
    if cmd.is_empty() {
        Action::Empty
    } else {
        Action::Run { cmd, cache }
    }
}

#[cfg(unix)]
fn elevate(cmd: &[String], preview: &str, cache: bool) -> i32 {
    unix::elevate(cmd, preview, cache)
}

// UAC has no credential cache like sudo's timestamp, so `--cache` is a no-op on
// Windows — every elevation prompts regardless.
#[cfg(windows)]
fn elevate(cmd: &[String], preview: &str, _cache: bool) -> i32 {
    win::elevate(cmd, preview)
}

#[cfg(target_os = "macos")]
fn setup_touch_id() -> i32 {
    macos::setup_touch_id()
}

#[cfg(not(target_os = "macos"))]
fn setup_touch_id() -> i32 {
    eprintln!("vudo: --setup-touch-id is macOS-only (Touch ID via pam_tid)");
    1
}

fn usage() {
    eprint!(
        "vudo — run a command as root with a graphical prompt that previews the
exact command first. Works on Linux, macOS, and Windows.

  vudo <command> [args...]

Examples:
  vudo pacman -Syu
  vudo systemctl restart nginx
  vudo rm -rf /var/tmp/junk

Options (only when they come first; otherwise treated as the command):
  -h, --help          show this help
  -V, --version       print the version
      --update        replace this binary with the latest release
      --setup-touch-id  enable Touch ID for sudo (macOS only)
  -c, --cache         reuse sudo's credential window; skip re-auth for a few
                      minutes instead of authorizing this one command

By default every command is authorized on its own (no cached credentials).
Everything else after \"vudo\" is the command that runs as root. Use \"--\" to
end option parsing if the command's own name starts with a dash.
"
    );
}

#[cfg(test)]
mod tests {
    use super::{classify, Action};

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_args() {
        assert!(matches!(classify(v(&[])), Action::Empty));
    }

    #[test]
    fn help_and_version() {
        assert!(matches!(classify(v(&["-h"])), Action::Help));
        assert!(matches!(classify(v(&["--help"])), Action::Help));
        assert!(matches!(classify(v(&["-V"])), Action::Version));
        assert!(matches!(classify(v(&["--version"])), Action::Version));
    }

    #[test]
    fn update_flag() {
        assert!(matches!(classify(v(&["--update"])), Action::Update));
    }

    #[test]
    fn setup_touch_id_flag() {
        assert!(matches!(
            classify(v(&["--setup-touch-id"])),
            Action::SetupTouchId
        ));
    }

    #[test]
    fn unknown_leading_option_is_rejected() {
        // These used to be treated as a command and elevated via sudo.
        assert!(matches!(
            classify(v(&["--updaet"])),
            Action::UnknownOption(_)
        ));
        assert!(matches!(
            classify(v(&["--helpp"])),
            Action::UnknownOption(_)
        ));
        assert!(matches!(classify(v(&["-x"])), Action::UnknownOption(_)));
    }

    #[test]
    fn a_command_runs_without_cache_by_default() {
        match classify(v(&["pacman", "-Syu"])) {
            Action::Run { cmd, cache } => {
                assert_eq!(cmd, v(&["pacman", "-Syu"]));
                assert!(!cache);
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn cache_flag_sets_cache_and_keeps_command() {
        for flag in ["-c", "--cache"] {
            match classify(v(&[flag, "systemctl", "restart", "nginx"])) {
                Action::Run { cmd, cache } => {
                    assert_eq!(cmd, v(&["systemctl", "restart", "nginx"]));
                    assert!(cache);
                }
                _ => panic!("expected Run"),
            }
        }
    }

    #[test]
    fn cache_with_no_command_is_empty() {
        assert!(matches!(classify(v(&["-c"])), Action::Empty));
    }

    #[test]
    fn double_dash_ends_options() {
        match classify(v(&["-c", "--", "--weird-tool", "arg"])) {
            Action::Run { cmd, cache } => {
                assert_eq!(cmd, v(&["--weird-tool", "arg"]));
                assert!(cache);
            }
            _ => panic!("expected Run"),
        }
        assert!(matches!(classify(v(&["--"])), Action::Empty));
    }
}
