#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Installs cua-spacesd and registers it with the platform service manager:
# systemd on Linux (packaging/linux/cua-spacesd.service), a GUI-session
# LaunchAgent on macOS (packaging/macos/com.trycua.spacesd.plist).
#
#   install.sh --binary ./cua-spacesd [--token TOKEN | --token-file F]
#   install.sh --version 0.1.0 ...        # download the release asset
#   install.sh --dry-run ...              # print what would happen
#
# The token is stored with mode 0600: /etc/cua/spacesd.env on Linux,
# ~/.cua/spacesd/token on macOS. Without a token the driver only listens
# on 127.0.0.1.
#
# Upgrading from an older name (cua-guestd, or cua-env-driver before it):
# its service is replaced, its token file and state directory are moved
# over, and `cua-guestd` and `cua-env-driver` stay as symlinks to
# `cua-spacesd` for one release.
set -euo pipefail

binary="" version="" token="" token_file="" dry_run=0 no_service=0
repo="${CUA_SPACESD_REPO:-${CUA_GUESTD_REPO:-${CUA_ENV_DRIVER_REPO:-trycua/cua}}}"
here="$(cd "$(dirname "$0")" && pwd)"

usage() { sed -n '2,14p' "$0"; exit "${1:-0}"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --binary) binary="$2"; shift 2 ;;
    --version) version="$2"; shift 2 ;;
    --token) token="$2"; shift 2 ;;
    --token-file) token_file="$2"; shift 2 ;;
    --no-service) no_service=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    -h|--help) usage 0 ;;
    *) echo "unknown argument: $1" >&2; usage 2 ;;
  esac
done

run() {
  if [ "$dry_run" = 1 ]; then printf '[dry-run] %s\n' "$*"; else "$@"; fi
}
write_file() { # path mode (content on stdin)
  if [ "$dry_run" = 1 ]; then printf '[dry-run] write %s (mode %s)\n' "$1" "$2"; cat >/dev/null; return; fi
  umask 077; cat >"$1"; chmod "$2" "$1"
}

os="$(uname -s)"; arch="$(uname -m)"
case "$arch" in x86_64|amd64) arch=x86_64 ;; arm64|aarch64) arch=aarch64 ;; *) echo "unsupported arch $arch" >&2; exit 1 ;; esac
[ -n "$token_file" ] && token="$(tr -d '\r\n' <"$token_file")"

if [ -z "$binary" ]; then
  [ -n "$version" ] || { echo "pass --binary PATH or --version VERSION" >&2; exit 2; }
  case "$os" in Linux) target="$arch-unknown-linux-gnu" ;; Darwin) target="$arch-apple-darwin" ;; *) echo "unsupported OS $os" >&2; exit 1 ;; esac
  url="https://github.com/$repo/releases/download/cua-spacesd-v$version/cua-spacesd-$target"
  binary="$(mktemp)"
  # The download is a temporary copy: removed however the script exits.
  trap 'rm -f "$binary"' EXIT
  # HTTPS only (redirects included); the published <asset>.sha256 must match.
  run curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$url" -o "$binary"
  if [ "$dry_run" = 0 ]; then
    expected="$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$url.sha256" | awk '{print $1; exit}')" ||
      { echo "could not download $url.sha256" >&2; exit 1; }
    if command -v sha256sum >/dev/null 2>&1; then actual="$(sha256sum "$binary" | awk '{print $1}')"; else actual="$(shasum -a 256 "$binary" | awk '{print $1}')"; fi
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
      echo "checksum mismatch for $url (expected ${expected:-none}, got $actual); aborting" >&2
      exit 1
    fi
  fi
fi

case "$os" in
  Linux)
    sudo=""; [ "$(id -u)" = 0 ] || sudo="sudo"
    run $sudo install -m 0755 "$binary" /usr/local/bin/cua-spacesd
    run $sudo mkdir -p /etc/cua
    for old in guestd env-driver; do
      run $sudo ln -sfn cua-spacesd "/usr/local/bin/cua-$old"
      # Older install: take over its token file and state directory.
      if [ -f "/etc/cua/$old.env" ] && [ ! -e /etc/cua/spacesd.env ]; then
        run $sudo mv "/etc/cua/$old.env" /etc/cua/spacesd.env
      fi
      if [ -d "/var/lib/cua/$old" ] && [ ! -e /var/lib/cua/spacesd ]; then
        run $sudo mv "/var/lib/cua/$old" /var/lib/cua/spacesd
      fi
    done
    if [ -n "$token" ]; then
      printf 'CUA_ENV_TOKEN=%s\n' "$token" | if [ "$dry_run" = 1 ]; then write_file /etc/cua/spacesd.env 0600; else $sudo sh -c 'umask 077; cat >/etc/cua/spacesd.env'; fi
    fi
    if [ "$no_service" = 0 ]; then
      for old in cua-guestd cua-env-driver; do
        unit="/etc/systemd/system/$old.service"
        if [ -f "$unit" ] && [ ! -L "$unit" ]; then
          run $sudo systemctl disable --now "$old.service" || true
          run $sudo rm -f "$unit"
        fi
      done
      run $sudo install -m 0644 "$here/linux/cua-spacesd.service" /etc/systemd/system/cua-spacesd.service
      run $sudo systemctl daemon-reload
      run $sudo systemctl enable --now cua-spacesd.service
    fi
    ;;
  Darwin)
    prefix="${CUA_SPACESD_PREFIX:-${CUA_GUESTD_PREFIX:-${CUA_ENV_DRIVER_PREFIX:-$HOME/.local}}}"
    for old in guestd env-driver; do
      if [ -d "$HOME/.cua/$old" ] && [ ! -e "$HOME/.cua/spacesd" ]; then
        run mv "$HOME/.cua/$old" "$HOME/.cua/spacesd"
      fi
    done
    run mkdir -p "$prefix/bin" "$HOME/.cua/spacesd" "$HOME/Library/LaunchAgents"
    run install -m 0755 "$binary" "$prefix/bin/cua-spacesd"
    run ln -sfn cua-spacesd "$prefix/bin/cua-guestd"
    run ln -sfn cua-spacesd "$prefix/bin/cua-env-driver"
    for old_plist in "$HOME/Library/LaunchAgents/com.trycua.guestd.plist" \
      "$HOME/Library/LaunchAgents/com.trycua.env-driver.plist"; do
      if [ "$no_service" = 0 ] && [ -f "$old_plist" ]; then
        run launchctl bootout "gui/$(id -u)" "$old_plist" 2>/dev/null || true
        run rm -f "$old_plist"
      fi
    done
    if [ -n "$token" ]; then
      printf '%s\n' "$token" | write_file "$HOME/.cua/spacesd/token" 0600
    fi
    if [ "$no_service" = 0 ]; then
      plist="$HOME/Library/LaunchAgents/com.trycua.spacesd.plist"
      sed -e "s#__PREFIX__#$prefix#g" -e "s#__HOME__#$HOME#g" "$here/macos/com.trycua.spacesd.plist" | write_file "$plist" 0644
      run launchctl bootout "gui/$(id -u)" "$plist" 2>/dev/null || true
      run launchctl bootstrap "gui/$(id -u)" "$plist"
    fi
    ;;
  *) echo "unsupported OS $os (use install.ps1 on Windows)" >&2; exit 1 ;;
esac
echo "cua-spacesd installed."
