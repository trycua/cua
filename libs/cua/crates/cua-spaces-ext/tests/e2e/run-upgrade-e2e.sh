#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Client/daemon skew between the released cua 0.7.0 (the `cua` in Cua Spaces
# 0.13.1) and this branch's `cua` (cua-spaces-cli): tests/upgrade_skew_live.rs
# with --ignored. No Space, no keychain, no relay: each case runs its daemons
# in a throwaway home (see the test).
#
#   run-upgrade-e2e.sh
#
# The 0.7.0 binary is CUA_UPGRADE_OLD_CUA when set. Otherwise the 0.13.1 app
# is downloaded once into ${XDG_CACHE_HOME:-~/.cache}/cua/upgrade-e2e, its
# checksum is checked, and `cua` is copied out of it. On macOS the DMG is
# mounted read-only; on Linux the .deb is unpacked. Nothing is installed.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../../../.." && pwd)"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/cua/upgrade-e2e"
mkdir -p "$cache"
base="https://github.com/trycua/cua/releases/download/cua-spaces-v0.13.1"

fetch() { # name sha256: the cached, checked download's path
  local f="$cache/$1"
  if [ ! -f "$f" ]; then
    curl -fsSL -o "$f.part" "$base/$1"
    mv "$f.part" "$f"
  fi
  if command -v sha256sum >/dev/null; then
    echo "$2  $f" | sha256sum -c - >/dev/null
  else
    echo "$2  $f" | shasum -a 256 -c - >/dev/null
  fi
  echo "$f"
}

old="${CUA_UPGRADE_OLD_CUA:-}"
if [ -z "$old" ]; then
  old="$cache/cua-0.7.0"
  if [ ! -x "$old" ]; then
    case "$(uname -s)-$(uname -m)" in
      Darwin-*)
        dmg="$(fetch cua-spaces-0.13.1-darwin-universal.dmg b5e9cbdc7ab42f976ad1050bfc167ce2bf2368de7b12f615e43c04c994916f42)"
        mnt="$(mktemp -d)"
        hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$dmg" >/dev/null
        cp "$mnt/Cua Spaces.app/Contents/MacOS/cua" "$old"
        hdiutil detach "$mnt" >/dev/null
        rmdir "$mnt"
        ;;
      Linux-x86_64 | Linux-aarch64)
        if [ "$(uname -m)" = x86_64 ]; then
          deb="$(fetch cua-spaces_0.13.1_amd64.deb b32d4c1d8f5ffd296beef43953dce3353c5da022aafb93883bbedbbfcbecb519)"
        else
          deb="$(fetch cua-spaces_0.13.1_arm64.deb 917bbe4fc19f8b285e2804f06d0f39d451b20607e2b21f47eb5a01e2c3d2cc36)"
        fi
        x="$(mktemp -d)"
        dpkg-deb -x "$deb" "$x"
        cp "$(find "$x" -type f -name cua -perm -u+x | head -1)" "$old"
        rm -rf "$x"
        ;;
      *) echo "set CUA_UPGRADE_OLD_CUA to the cua of Cua Spaces 0.13.1" >&2; exit 2 ;;
    esac
    chmod +x "$old"
  fi
fi
"$old" --version | grep -q '^cua 0\.7\.0' || { echo "$old is not cua 0.7.0" >&2; exit 2; }

cd "$repo/libs/cua"
cargo build -p cua-spaces-cli
target="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
# Short: the daemons' Unix sockets live under it.
tmp="$(mktemp -d "${TMPDIR:-/tmp}/cuaup.XXXX")"
trap 'rm -rf "$tmp"' EXIT
CUA_UPGRADE_OLD_CUA="$old" CUA_UPGRADE_NEW_CUA="$target/debug/cua-spaces-cli" CUA_UPGRADE_E2E_TMP="$tmp" \
  cargo test -p cua-spaces-ext --test upgrade_skew_live -- --ignored --test-threads=1 "$@"
