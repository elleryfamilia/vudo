# vudo

A single small Rust binary that runs one command as root behind a graphical
prompt. It does not draw its own password field: it delegates authentication to
each platform's native agent (sudo's askpass on Unix, Touch ID on macOS, UAC on
Windows) and shows a preview of the exact command first.

## Layout

| Path | What lives there |
|------|------------------|
| `src/main.rs` | Entry point: argument classification (`classify`), `usage()`, the per-OS `elevate`/`setup_touch_id` shims, and the CLI tests |
| `src/quote.rs` | Turning an argv vector into the display preview; POSIX single-quoting |
| `src/dialog.rs` | `info_block()` — the message body every dialog backend shows, so the wording stays identical across platforms |
| `src/caller.rs` | Best-effort name of whoever invoked vudo (nearest ancestor that isn't a shell or pass-through wrapper) plus the controlling-terminal check |
| `src/icon.rs` | Embedded brand mark and fingerprint glyph, written to a cache file so dialog tools can point at it |
| `src/unix.rs` | Shared Linux/macOS elevation over `sudo -A`; the `__askpass` helper mode; the temp askpass wrapper script |
| `src/linux.rs` | zenity → kdialog → pinentry dialogs, the fingerprint notice driven by fprintd D-Bus signals, and the PAM-stack sniffing in `silent_auth_hint()` |
| `src/macos.rs` | osascript dialogs, Touch ID detection, `--setup-touch-id` (adds the `pam_tid` line to `/etc/pam.d/sudo_local` when sudo includes that file, otherwise to `/etc/pam.d/sudo`) |
| `src/win.rs` | UAC elevation via PowerShell `Start-Process -Verb RunAs` |
| `src/update.rs` | `vudo --update`: resolve the latest tag, download, verify SHA-256, replace the binary in place |
| `install.sh`, `install.ps1` | The `curl \| sh` and `irm \| iex` installers; also the fallback path used by Windows self-update |
| `.github/workflows/ci.yml` | Build and test on Linux/macOS/Windows, plus fmt and clippy |
| `.github/workflows/release.yml` | Tag-triggered build of the five release targets and their checksum files |
| `assets/` | README artwork. `icon.png` and `fingerprint.png` are compiled into the binary with `include_bytes!` in `src/icon.rs`; the rest (`hero.svg`, the wordmarks) are README-only |

Platform code is selected with `#[cfg(...)]` in `src/main.rs`; nothing outside
those gates is allowed to assume a specific OS. `libc` is the only dependency and
only on Unix — no crate is pulled in on Windows, and `Cargo.toml` says so. Keep
it that way: downloads, extraction and checksums shell out to `curl`/`wget`/`tar`/
`sha256sum` instead of adding crates.

The release profile is tuned for binary size, not speed (`strip`, `lto`,
`opt-level = "z"`, `panic = "abort"`, `codegen-units = 1`). Edition is 2021 and
the declared MSRV is Rust 1.74.

## Commands

```sh
cargo build              # debug build
cargo build --release    # the shipped binary
cargo test               # the full test suite
cargo fmt --all --check  # formatting (CI fails on any diff)
cargo clippy --all-targets -- -D warnings   # lints are errors
```

`cargo test`, `cargo fmt --check` and `cargo clippy -D warnings` are exactly
what CI runs, so run all three before calling a change done. There is no
`rustfmt.toml` or `clippy.toml` to override the defaults, and no task file or
script runner — the manifests and the two workflows are the whole configuration.

Cross-target builds are only done in the release workflow, for the five
published targets:

```sh
cargo build --release --target x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-pc-windows-msvc
```

The Linux builds are musl static binaries. No separate typecheck command exists;
`cargo build` and `cargo clippy` in CI cover it, and `cargo check` gives the
faster local answer.

## Tests

Tests are inline `#[cfg(test)] mod tests` blocks at the bottom of the module
they belong to — there is no `tests/` directory and no test-only dependency. They
must not need the network, root, a GUI, or an interactive session: they cover
parsing, quoting, argument classification, version comparison, PAM-line
recognition and the askpass wrapper's shape. Anything that actually elevates is
verified by hand on a real machine, and the README's Linux/macOS/Windows
behaviour notes are the record of that.

## Code conventions

- Every source file opens with a `//!` module comment explaining what the module
  is for and *why* it works that way. Non-obvious decisions in the body get the
  same treatment — several comments exist to record a specific bug (why the
  timestamp reset inherits stdin, why the temp dir name carries nanos and a
  counter, why the askpass script assigns and exports on separate lines). Match
  that style: explain the reason, not the mechanics.
- The command to run is passed as an argv vector to `Command`, never a shell
  string. Shell quoting exists only for the preview (`quote::preview`); the same
  rule applies to the PowerShell and AppleScript strings, which are escaped by
  `win::ps_quote` / `macos::apple_text` / `macos::apple_str`.
- The password is written to stdout for sudo to read and appears nowhere else —
  not in argv, not in the environment, not on disk, not in a log. Do not add
  logging around the askpass path.
- Errors go to stderr as one lowercase line prefixed `vudo: `, and the process
  exits with a meaningful code: `0` success, `1` failure, `2` usage error or
  unknown leading option, `127` command could not be run, `130` the user
  cancelled. Never fail silently.
- External GUI tools are probed with `have(bin)` and fall back in a fixed order
  (zenity → kdialog → pinentry, yad → zenity → kdialog for the fingerprint
  notice). New backends are added to the end of that chain, never by removing a
  fallback.
- Temporary files and directories are given unique names, created `0700`, and
  cleaned up by a `Drop` guard (`AskpassWrapper`, `TmpGuard`).
- The options are deliberately narrow: `-h/--help`, `-V/--version`, `--update`,
  `--setup-touch-id`, `-c/--cache`, and `--` to end option parsing. An
  unrecognized leading option is rejected rather than run as a command.
- No `CHANGELOG.md` or `CONTRIBUTING.md`. Commit subjects use a type prefix
  (`feat:`, `fix:`, `docs:`, `chore:`) and, for merged pull requests, end with
  the PR number (`fix: ... (#22)`). Pull requests are squashed: the history is
  linear, with no merge commits.

## Release

The release is cut by pushing a `v*` tag. `release.yml` then creates the GitHub
release first and only afterwards uploads assets, so parallel build jobs cannot
race to create it.

What must be current before the tag:

1. A clean working tree on `main`, with CI green — the same build/test/fmt/clippy
   jobs must pass, since nothing else gates the release.
2. `version` in `Cargo.toml` **and** the `vudo` entry in `Cargo.lock` set to the
   new version. Every tag in this repository has a matching version in the tagged
   `Cargo.toml`; `vudo --version` and the self-update comparison read it from
   there (`env!("CARGO_PKG_VERSION")`).
3. `README.md` matching the shipped behaviour. It is the only user-facing
   documentation, and the binaries ship with `README.md` and `LICENSE` inside
   each archive.
4. The release commit merged to `main` and pushed. The tag is cut from a commit
   that exists on `main`, and the installers fetch from `main`, so a change that
   never landed is a change users never see.

Steps:

```sh
git switch main
git pull --ff-only
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test

# bump the version in Cargo.toml and Cargo.lock
cargo build --release          # refreshes Cargo.lock; then review `git diff`
git commit -am "chore: release vX.Y.Z"
git push origin main

git tag -a vX.Y.Z -m "vX.Y.Z — <one-line summary>"   # tags are annotated
git push origin vX.Y.Z
```

The tag name must equal `v` + the `Cargo.toml` version. Tag messages follow the
existing form: `vX.Y.Z — short summary of what changed`.

Pushing the tag publishes five assets, each with a sibling `.sha256`:
`vudo-vX.Y.Z-<target>.tar.gz` for the four Unix targets and
`vudo-vX.Y.Z-x86_64-pc-windows-msvc.zip` for Windows. That asset name shape is
consumed in three other places, so a change to the target list or naming must be
made in all of them together: `install.sh`, `install.ps1`, and
`src/update.rs` (`platform_target`).

There is no post-release step in the repository: the release workflow builds,
packages and uploads in one pass. Check the published release page for the five
archives plus checksums, then confirm `vudo --version` reports the new tag.
