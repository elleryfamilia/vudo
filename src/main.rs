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

    // Reserved vudo options (only when they lead — everything else is the
    // command to run as root).
    match args.first().map(String::as_str) {
        None => {
            usage();
            std::process::exit(2);
        }
        Some("-h") | Some("--help") => {
            usage();
            std::process::exit(0);
        }
        Some("-V") | Some("--version") => {
            println!("vudo {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        Some("--update") => std::process::exit(update::run()),
        _ => {}
    }

    let preview = quote::preview(&args);
    std::process::exit(elevate(&args, &preview));
}

#[cfg(unix)]
fn elevate(cmd: &[String], preview: &str) -> i32 {
    unix::elevate(cmd, preview)
}

#[cfg(windows)]
fn elevate(cmd: &[String], preview: &str) -> i32 {
    win::elevate(cmd, preview)
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
  -h, --help       show this help
  -V, --version    print the version
      --update     replace this binary with the latest release

Everything else after \"vudo\" is the command that runs as root.
"
    );
}
