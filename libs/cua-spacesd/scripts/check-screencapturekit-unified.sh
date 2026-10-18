#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Guard against the `sc_stream_create` null-function-pointer segfault.
#
# WHY THIS EXISTS
# ---------------
# The `screencapturekit` crate ships a Swift bridge that it compiles, per
# crate version, into a static archive that is ALWAYS named
# `libScreenCaptureKitBridge.a`, and that archive exports unmangled C symbols
# via `@_cdecl` -- `sc_stream_create`, `sc_stream_start_capture`, and friends.
# Those names carry no version, so if two versions of the crate end up in one
# binary the linker resolves every caller to whichever archive it reaches
# first. Exactly one definition survives and it silently serves both callers.
#
# That is only a latent problem until the two versions disagree about a
# signature, and 6.x and 8.x do:
#
#   6.1.0  sc_stream_create(filter, config, context, err_cb, sample_cb)
#   8.0.1  sc_stream_create(filter, config, context, err_cb, sample_cb,
#                           context_retain, context_release)
#
# 8.x's implementation calls `context_retain(context)` unconditionally while
# constructing its delegate. When 6.x's five-argument call site is bound to
# 8.x's seven-parameter implementation, `context_retain` is read out of an
# argument register the caller never set. In practice that register is zero,
# so the call branches to 0x0 and the process dies with
# `EXC_BAD_ACCESS (SIGSEGV), KERN_INVALID_ADDRESS at 0x0`, with `createStream`
# on top of the stack. This killed `cua-spacesd` on every `open_session` that
# started a window capture.
#
# IMPORTANT: the linker does NOT warn about this. Each version's build script
# emits its own `-L <out-dir>` plus a `-lScreenCaptureKitBridge`, so from the
# linker's point of view nothing is duplicated -- it just never opens the
# second archive. A duplicate-symbol grep over the build log (which is what
# cua-driver's `test-swift-linker-symbols.sh` does) stays green through this
# bug. Hence this separate, graph-level check.
#
# WHAT IT ENFORCES
#   1. The resolved dependency graph contains exactly one `screencapturekit`.
#   2. If a release build tree is present, exactly one Swift bridge archive
#      was built into it.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

fail=0

# --- 1. dependency graph ---------------------------------------------------
# Read the lockfile rather than `cargo tree` so the check needs no network and
# no target-specific resolution, and so it reports every version it finds.
# Plain word-splitting rather than `mapfile`: macOS ships bash 3.2, so this
# script must not use bash 4 builtins or it fails on exactly the platform whose
# bug it guards.
versions="$(
  awk '
    /^name = "screencapturekit"$/ { want = 1; next }
    want && /^version = / { gsub(/[",]/, "", $3); print $3; want = 0 }
  ' Cargo.lock | sort -u
)"
# shellcheck disable=SC2206
version_list=(${versions})

echo "screencapturekit versions in Cargo.lock: ${versions:-<none>}"

if [[ "${#version_list[@]}" -eq 0 ]]; then
  echo "FAIL: no screencapturekit in Cargo.lock -- the macOS capture path needs it." >&2
  fail=1
elif [[ "${#version_list[@]}" -gt 1 ]]; then
  cat >&2 <<EOF
FAIL: ${#version_list[@]} versions of screencapturekit in the dependency graph: ${versions}

Two versions cannot coexist in one binary: their Swift bridges export the same
unmangled C symbols from identically-named static archives, so one version's
call sites get bound to the other version's implementations. For the 6.x/8.x
pair this crashes cua-spacesd at 0x0 inside sc_stream_create on every window capture.

Align the 'screencapturekit' dependency in
  crates/cua-spacesd-desktop/Cargo.toml
with the one used by
  ../cua-driver/rust/crates/platform-macos/Cargo.toml
and re-run 'cargo update -p screencapturekit'.
EOF
  fail=1
else
  echo "PASS: single screencapturekit version (${version_list[0]}) in the dependency graph."
fi

# --- 2. built Swift bridge archives ----------------------------------------
# Belt and braces: even with one crate version, a stale or vendored second
# bridge in the target dir would reintroduce the ambiguity.
target_dir="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}"
if [[ -d "${target_dir}/release/build" ]]; then
  archive_list="$(find "${target_dir}/release/build" -name libScreenCaptureKitBridge.a 2>/dev/null | sort)"
  # shellcheck disable=SC2206
  archives=(${archive_list})
  if [[ "${#archives[@]}" -gt 1 ]]; then
    echo "FAIL: ${#archives[@]} ScreenCaptureKit Swift bridge archives in ${target_dir}:" >&2
    printf '  %s\n' "${archives[@]}" >&2
    echo "Only one may be linked. Remove the build tree and rebuild." >&2
    fail=1
  elif [[ "${#archives[@]}" -eq 1 ]]; then
    echo "PASS: exactly one Swift bridge archive built."
  fi
else
  echo "SKIP: no release build tree at ${target_dir}; graph check only."
fi

exit "${fail}"
