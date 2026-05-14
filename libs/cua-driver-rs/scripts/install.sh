#!/usr/bin/env bash
# cua-driver-rs installer — download the latest cua-driver-rs release tarball
# from GitHub Releases and drop the binary into ~/.local/bin (or a path given
# via --bin-dir / CUA_DRIVER_RS_BIN_DIR).  Sudo-free.
#
# This is the Rust port of cua-driver — cross-platform (macOS / Linux / Windows
# via WSL or git-bash) computer-use automation. The Swift cua-driver (macOS
# only) ships separately under tag prefix `cua-driver-v*` and is installed
# via `libs/cua-driver/scripts/install.sh`; this script is hard-pinned to
# `cua-driver-rs-v*` and will never pick up the Swift binary.
#
# Usage:
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver-rs/scripts/install.sh)"
#
# Flags:
#   --bin-dir <path>     install the binary to <path> instead of ~/.local/bin
#   --no-modify-path     skip auto-appending an `export PATH=...` line
#
# Env overrides:
#   CUA_DRIVER_RS_VERSION=0.1.2     pin a specific release tag
#   CUA_DRIVER_RS_BIN_DIR=PATH      same as --bin-dir
#   CUA_DRIVER_RS_NO_MODIFY_PATH=1  same as --no-modify-path
#
# ⚠️  This is a BETA release. The Rust port is feature-complete on Windows
# and Linux; macOS parity with the Swift cua-driver is in progress. For
# production macOS automation prefer `libs/cua-driver/scripts/install.sh`.
set -euo pipefail

REPO="trycua/cua"
BINARY_NAME="cua-driver"
TAG_PREFIX="cua-driver-rs-v"
BIN_DIR="${CUA_DRIVER_RS_BIN_DIR:-$HOME/.local/bin}"
NO_MODIFY_PATH="${CUA_DRIVER_RS_NO_MODIFY_PATH:-0}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bin-dir) BIN_DIR="$2"; shift 2 ;;
        --bin-dir=*) BIN_DIR="${1#*=}"; shift ;;
        --no-modify-path) NO_MODIFY_PATH=1; shift ;;
        *) shift ;;
    esac
done

BIN_LINK="$BIN_DIR/$BINARY_NAME"
TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

log() { printf '==> %s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

# --- Resolve OS/arch ----------------------------------------------------

OS=$(uname -s)
ARCH_RAW=$(uname -m)
case "$OS-$ARCH_RAW" in
    Darwin-arm64|Darwin-aarch64)     LABEL="darwin-arm64"   ;;
    Darwin-x86_64)                   LABEL="darwin-x86_64"  ;;
    Linux-x86_64|Linux-amd64)        LABEL="linux-x86_64"   ;;
    *)
        err "unsupported platform: $OS / $ARCH_RAW"
        err "  cua-driver-rs ships prebuilts for: darwin-arm64, darwin-x86_64, linux-x86_64."
        err "  Windows users should download cua-driver-rs-<v>-windows-x86_64.zip from GitHub Releases directly."
        exit 1
        ;;
esac

for cmd in curl tar; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        err "$cmd not found on PATH"
        exit 1
    fi
done

# --- Resolve release tag ------------------------------------------------

if [[ -n "${CUA_DRIVER_RS_VERSION:-}" ]]; then
    TAG="${TAG_PREFIX}${CUA_DRIVER_RS_VERSION#v}"
    log "using version from CUA_DRIVER_RS_VERSION: $TAG"
else
    log "resolving latest $TAG_PREFIX* release via GitHub API"
    # Pinned to the exact `cua-driver-rs-v*` prefix so this script can never
    # accidentally pick up a Swift `cua-driver-v*` release.
    TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=40" \
        | grep -Eo '"tag_name":[[:space:]]*"'"${TAG_PREFIX}"'[^"]+"' \
        | sed -E 's/.*"'"${TAG_PREFIX}"'([0-9]+[.][0-9]+[.][0-9]+)"/\1/' \
        | sort -t. -k1,1nr -k2,2nr -k3,3nr \
        | head -n 1 \
        | sed -E 's/^/'"${TAG_PREFIX}"'/')
    if [[ -z "$TAG" ]]; then
        err "no release matching ${TAG_PREFIX}* found on $REPO"
        err "  (cua-driver-rs is a BETA-stage cross-platform port; releases may not be published yet.)"
        exit 1
    fi
    log "latest release: $TAG"
fi

VERSION="${TAG#${TAG_PREFIX}}"

# --- Download bare-binary tarball ---------------------------------------

# Prefer the bare-binary tarball (single `cua-driver` file at the root) —
# the directory tarball would require unpacking and copying, but the bare
# form is curl-pipe-able.
#
# macOS: the release workflow publishes one universal binary (arm64 + x86_64
# lipo'd together) named `darwin-universal-binary`.  There are NO per-arch
# bare-binary tarballs for macOS — only the directory tarballs are split
# by arch (darwin-arm64 / darwin-x86_64).  The universal binary works on
# both Apple Silicon and Intel, so we always fetch it on macOS.
#
# Linux: per-arch bare-binary tarballs exist (e.g. linux-x86_64-binary).
case "$LABEL" in
    darwin-*) TARBALL="cua-driver-rs-${VERSION}-darwin-universal-binary.tar.gz" ;;
    *)        TARBALL="cua-driver-rs-${VERSION}-${LABEL}-binary.tar.gz" ;;
esac
URL="https://github.com/$REPO/releases/download/$TAG/$TARBALL"

log "downloading $URL"
if ! curl -fsSL -o "$TMP_DIR/$TARBALL" "$URL"; then
    err "download failed; try CUA_DRIVER_RS_VERSION=<version> to pin a specific release"
    exit 1
fi

log "extracting"
tar -xzf "$TMP_DIR/$TARBALL" -C "$TMP_DIR"

# The bare-binary tarball contains exactly `cua-driver` at the root.
SRC="$TMP_DIR/$BINARY_NAME"
if [[ ! -f "$SRC" ]]; then
    err "expected $BINARY_NAME in tarball but didn't find it"
    ls -la "$TMP_DIR"
    exit 1
fi

# --- Install ------------------------------------------------------------

mkdir -p "$BIN_DIR"
install -m 0755 "$SRC" "$BIN_LINK"
log "installed $BIN_LINK (version $VERSION)"

# Auto-extend PATH for users whose shell doesn't already include BIN_DIR.
if [[ "$NO_MODIFY_PATH" != "1" ]] && [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    SHELL_RC=""
    case "${SHELL:-}" in
        */zsh)  SHELL_RC="$HOME/.zshrc"  ;;
        */bash) SHELL_RC="$HOME/.bashrc" ;;
    esac
    if [[ -n "$SHELL_RC" ]]; then
        {
            printf '\n# Added by cua-driver-rs installer — see https://github.com/trycua/cua\n'
            printf 'export PATH="%s:$PATH"\n' "$BIN_DIR"
        } >> "$SHELL_RC"
        log "appended PATH update to $SHELL_RC — open a new shell or run \`source $SHELL_RC\`"
    else
        log "WARNING: $BIN_DIR is not on PATH; add it manually."
    fi
fi

echo ""
echo "cua-driver-rs $VERSION installed."
echo ""
echo "Try it:"
echo "  $BIN_LINK list-tools"
echo "  $BIN_LINK list_apps"
echo ""
echo "Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver-rs"
echo ""
echo "⚠️  BETA: cua-driver-rs is a cross-platform Rust port of the Swift"
echo "    cua-driver. Windows and Linux support is feature-complete; macOS"
echo "    parity with the Swift binary is in progress. For production macOS"
echo "    use, prefer the original install:"
echo "      /bin/bash -c \"\$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh)\""
