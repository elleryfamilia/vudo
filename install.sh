#!/bin/sh
# vudo installer — downloads the prebuilt binary for your OS/arch from the
# latest GitHub release, verifies its checksum, and installs it.
#
#   curl -fsSL https://raw.githubusercontent.com/elleryfamilia/vudo/main/install.sh | sh
#
# Env overrides:
#   VUDO_VERSION       install a specific tag (default: latest)
#   VUDO_INSTALL_DIR   where to put the binary (default: ~/.local/bin)
set -eu

REPO="elleryfamilia/vudo"
INSTALL_DIR="${VUDO_INSTALL_DIR:-$HOME/.local/bin}"

err()  { printf 'vudo: error: %s\n' "$1" >&2; exit 1; }
info() { printf 'vudo: %s\n' "$1" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

# --- detect platform ---
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)
    case "$arch" in
      x86_64 | amd64) target="x86_64-unknown-linux-musl" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-musl" ;;
      *) err "unsupported Linux arch '$arch' — build from source: cargo install --git https://github.com/$REPO" ;;
    esac ;;
  Darwin)
    case "$arch" in
      x86_64) target="x86_64-apple-darwin" ;;
      arm64 | aarch64) target="aarch64-apple-darwin" ;;
      *) err "unsupported macOS arch '$arch'" ;;
    esac ;;
  *)
    err "unsupported OS '$os' — on Windows download the .zip from https://github.com/$REPO/releases" ;;
esac

# --- downloader ---
if have curl; then
  fetch() { curl -fsSL -o "$2" "$1"; }
  fetch_stdout() { curl -fsSL "$1"; }
elif have wget; then
  fetch() { wget -qO "$2" "$1"; }
  fetch_stdout() { wget -qO- "$1"; }
else
  err "need curl or wget"
fi

# --- resolve tag ---
tag="${VUDO_VERSION:-}"
if [ -z "$tag" ]; then
  tag="$(fetch_stdout "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | head -1 \
    | sed 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/')"
fi
[ -n "$tag" ] || err "could not determine the latest release tag (no releases yet?)"

asset="vudo-${tag}-${target}.tar.gz"
url="https://github.com/$REPO/releases/download/${tag}/${asset}"

info "installing ${tag} (${target})"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

fetch "$url" "$tmp/$asset" || err "download failed: $url"
fetch "$url.sha256" "$tmp/$asset.sha256" || err "checksum download failed: $url.sha256"

# --- verify checksum ---
if have sha256sum; then
  sum() { sha256sum "$1" | awk '{print $1}'; }
elif have shasum; then
  sum() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  sum() { echo ""; }
  info "warning: no sha256 tool found — skipping checksum verification"
fi
expected="$(awk '{print $1}' "$tmp/$asset.sha256")"
actual="$(sum "$tmp/$asset")"
if [ -n "$actual" ] && [ "$expected" != "$actual" ]; then
  err "checksum mismatch (expected $expected, got $actual)"
fi

# --- extract & install ---
tar xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
src="$tmp/vudo-${tag}-${target}/vudo"
if have install; then
  install -m 0755 "$src" "$INSTALL_DIR/vudo"
else
  cp "$src" "$INSTALL_DIR/vudo" && chmod 0755 "$INSTALL_DIR/vudo"
fi

info "installed to $INSTALL_DIR/vudo"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) : ;;
  *)
    info "note: $INSTALL_DIR is not on your PATH; add it, e.g.:"
    info "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.profile" ;;
esac

info "run 'vudo --help' to get started"
