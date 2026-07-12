# vudo

Run a command as root with a graphical prompt that **previews the exact
command** before you authorize it. A single dependency-free binary for Linux,
macOS, and Windows.

Built for contexts with no TTY to type a password into — an AI coding agent, a
hotkey, a `!` shell escape. Instead of a terminal prompt, `vudo` pops your OS's
native auth prompt showing what will run.

```
vudo <command> [args...]
```

## Examples

```sh
vudo pacman -Syu
vudo systemctl restart nginx
vudo rm -rf /var/tmp/junk
```

Everything after `vudo` is the command run as root. There are no options except
`--help`.

## How it works

`vudo` doesn't implement its own password field — it delegates authentication
to each OS's native agent, which is more secure and familiar:

| Platform | Mechanism |
|----------|-----------|
| Linux    | `sudo -A` with an askpass helper backed by **zenity → kdialog → pinentry** |
| macOS    | `sudo` via **Touch ID** (`pam_tid`) when configured, else an **osascript** password dialog |
| Windows  | **UAC** via PowerShell `Start-Process -Verb RunAs`, with the preview shown as a message box first |

On Unix it uses `sudo -A` (askpass) rather than `sudo -S` (password on stdin),
so the command's stdin/tty stay free — interactive root commands like
`pacman -Syu` ("Proceed? [Y/n]") still work. The password goes from the dialog
straight to sudo; it never touches argv, the environment, disk, or a log. If
sudo credentials are already cached, `vudo` runs straight through with no
prompt.

**Notes**

- Linux: install `zenity` (or `kdialog`) for the most reliable dialog; pinentry
  is a last-resort fallback and can be flaky under some compositors.
- Windows: UAC shows its own consent prompt and runs the command in a separate
  elevated process, so the command's output isn't captured — only its exit code
  is returned. The preview is a separate message box shown before UAC.

## Install

Prebuilt binaries are published on the [releases page]. Or build from source:

```sh
cargo install --path .        # from a clone
# or
cargo install vudo            # from crates.io (once published)
```

Drop the resulting `vudo` binary anywhere on your `PATH`.

## Build & test

```sh
cargo build --release
cargo test            # offline: Assuan/pinentry parsing + quoting
```

CI builds and tests on Linux, macOS, and Windows.

## License

MIT

[releases page]: https://github.com/elleryfamilia/vudo/releases
