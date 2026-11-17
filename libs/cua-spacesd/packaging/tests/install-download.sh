#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Hermetic test: packaging/install.sh --version checks the published
# <asset>.sha256 before installing. A fake `curl` on PATH serves the files;
# HOME and the prefix are temp dirs, and every case runs with --no-service.
# The success case installs only on macOS (Linux installs need sudo).
set -u
here="$(cd "$(dirname "$0")" && pwd)"
installer="$here/../install.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fail=0

mkdir -p "$work/bin" "$work/srv"
printf 'spacesd-binary\n' >"$work/srv/bin"
cat >"$work/bin/curl" <<'CURL'
#!/bin/sh
# Fake curl: records the protocol flags, serves $FAKE_SRV/bin or /sum.
out=""; url=""
for a in "$@"; do
  case "$prev" in -o) out="$a" ;; esac
  case "$a" in https://*|http://*) url="$a" ;; esac
  prev="$a"
done
case " $* " in *" --proto =https "*) ;; *) echo "curl called without --proto =https" >&2; exit 9 ;; esac
case "$url" in
  *.sha256) cat "$FAKE_SRV/sum" ;;
  *) cp "$FAKE_SRV/bin" "$out" ;;
esac
CURL
chmod +x "$work/bin/curl"

run() { # expected-sum
  printf '%s  cua-spacesd\n' "$1" >"$work/srv/sum"
  home="$work/home.$RANDOM"
  mkdir -p "$home"
  env -i PATH="$work/bin:$PATH" HOME="$home" FAKE_SRV="$work/srv" CUA_SPACESD_PREFIX="$home/prefix" \
    bash "$installer" --version 9.9.9 --no-service >"$work/out" 2>&1
}
check() { if "$@"; then echo "ok   $name"; else echo "FAIL $name"; sed 's/^/     | /' "$work/out"; fail=1; fi; }

run 0000000000000000000000000000000000000000000000000000000000000000; status=$?
name="checksum mismatch aborts before installing"
check sh -c "[ $status != 0 ] && grep -q 'checksum mismatch' '$work/out' && [ ! -e '$home/prefix/bin/cua-spacesd' ]"

if [ "$(uname -s)" = Darwin ]; then
  good="$(shasum -a 256 "$work/srv/bin" | awk '{print $1}')"
  run "$good"; status=$?
  name="matching checksum installs"
  check sh -c "[ $status = 0 ] && cmp -s '$work/srv/bin' '$home/prefix/bin/cua-spacesd'"
fi
exit "$fail"
