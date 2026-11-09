#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# build-macos-golden.sh — the CI wrapper around the Cua Spaces macOS golden.
#
# This script owns ONLY what is specific to running the build on a CI runner:
#
#   * preflighting the ghcr push credentials before the hour-plus build
#   * creating the VM (clone the base image, boot it, wait for ssh)
#   * staging the signed artifacts CI just produced where the golden build
#     expects to find them
#   * `lume push`
#
# It owns NOTHING about what a golden CONTAINS. That is defined in exactly one
# place — scripts/golden/build-golden.sh — and this script delegates the whole
# provisioning phase to it.
#
# WHY, because this was a real and expensive bug: this file used to be a second,
# parallel definition of the golden. It provisioned the driver, the old
# capture and command daemons, node and Chrome, and stopped there. The published Spaces
# golden also has Unity, Blender + blender-mcp, the agent stack, the skills MCP,
# the coding-agent CLI, the whole ~/.cua payload (per-boot prepare-space,
# write-agent-mcp, run-agent, the permission approver, the lazy-app MCP shim)
# and, last, the sanitizer that strips the build machine's identity. Publishing
# this script's output to the tag the app pulls would have reported a successful
# release and shipped an image with no Unity, no Blender MCP, no skills, no
# per-boot prep and no sanitizer gate. The same reasoning already de-duplicated
# the Chrome install; this applies it to the rest.
#
# Two mechanical guards keep it from happening again:
#   golden/check-golden-coverage.sh  static, VM-free; asserts this script still
#                                    delegates and has not re-grown its own
#                                    provisioning. Runs below, before anything
#                                    expensive.
#   golden/verify-golden.sh          runs in the guest as the LAST build step,
#                                    after the sanitizer, and fails the build if
#                                    the image is missing anything a Space needs
#                                    (golden/golden-required.txt).
#
# REQUIREMENTS (host): Apple Silicon macOS with `lume` + a running `lume serve`.
# The TCC seed only works inside a SIP-disabled VirtualMac, so this must run on a
# real Apple-Silicon macOS host (self-hosted CI runner), never GitHub-hosted.
#
# The CuaDriverLocal.app and Cua Spacesd.app bundles must already be built AND
# code-signed with a stable, cert-backed identity (a self-signed dev cert for
# local use, or the Developer ID cert in CI — ad-hoc signatures do NOT hold the
# seeded TCC grants). Pass their paths via DRIVER_APP / SPACESD_APP.
# cua-spacesd is the only in-guest daemon: it also receives teleports and
# files (TeleportService), so there is no separate receiver binary any more.
set -euo pipefail

# --- config (env-overridable) -----------------------------------------------
BASE_IMAGE="${BASE_IMAGE:-macos-tahoe}"        # base VM to clone (SIP-off, autologin, ssh)
GOLDEN="${GOLDEN:-cua-spaces-golden}"          # golden VM name to (re)build
DRIVER_APP="${DRIVER_APP:?set DRIVER_APP=/path/to/CuaDriverLocal.app (signed)}"
SPACESD_APP="${SPACESD_APP:-${ENV_DRIVER_APP:-}}"  # ENV_DRIVER_APP: pre-rename name
SPACESD_APP="${SPACESD_APP:?set SPACESD_APP=/path/to/Cua Spacesd.app (signed; libs/cua-spacesd/scripts/build-macos-app.sh)}"
PUSH_IMAGE="${PUSH_IMAGE:-}"                    # e.g. macos-tahoe-cua-spaces:26.3.0 ; empty = build only
PUSH_REGISTRY="${PUSH_REGISTRY:-ghcr.io}"
PUSH_ORG="${PUSH_ORG:-trycua}"
LUME_SSH_PASSWORD="${LUME_SSH_PASSWORD:-lume}"
REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)}"
GOLDEN_SRC="$REPO_ROOT/apps/cua-spaces/scripts/golden"
LUME_API="${LUME_API:-http://127.0.0.1:7777}"

vmip() { curl -s "$LUME_API/lume/vms/$GOLDEN" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("ipAddress") or "")'; }

# --- fast gates, before anything expensive ----------------------------------
# Milliseconds. The alternative is learning an hour in, or — worse — at publish.
echo "==> Checking golden coverage (CI path vs. the single golden definition)"
bash "$GOLDEN_SRC/check-golden-coverage.sh"

# Preflight the push credentials BEFORE the hour-plus build, not after it.
# `lume push` reads GITHUB_USERNAME/GITHUB_TOKEN (or GHCR_*) itself and ignores
# docker's credential store, so a `docker login` upstream proves nothing. Losing
# a whole golden build to PushError.authenticationFailed at the last step is the
# expensive way to learn the token was missing or lacked write:packages.
if [ -n "$PUSH_IMAGE" ]; then
  echo "==> Preflight: ghcr push credentials for $PUSH_ORG"
  : "${GITHUB_USERNAME:=${GHCR_USERNAME:-}}"
  : "${GITHUB_TOKEN:=${GHCR_TOKEN:-}}"
  if [ -z "$GITHUB_USERNAME" ] || [ -z "$GITHUB_TOKEN" ]; then
    echo "FATAL: PUSH_IMAGE is set but GITHUB_USERNAME/GITHUB_TOKEN are not." >&2
    echo "       lume push authenticates from those env vars; docker login is not used." >&2
    exit 1
  fi
  # Ask ghcr for a push-scoped token now. A read-only PAT gets a token back that
  # silently lacks the push scope, so check the returned scope, not just the 200.
  _img="${PUSH_IMAGE%%:*}"
  _scope="repository:$PUSH_ORG/$_img:pull,push"
  _resp="$(curl -sS -u "$GITHUB_USERNAME:$GITHUB_TOKEN" \
    "https://ghcr.io/token?scope=$_scope&service=ghcr.io" 2>/dev/null || true)"
  if ! printf '%s' "$_resp" | grep -q '"token"'; then
    echo "FATAL: ghcr.io refused the credentials for $PUSH_ORG/$_img." >&2
    echo "       response: $_resp" >&2
    exit 1
  fi
  echo "    ok: ghcr.io issued a pull,push token for $PUSH_ORG/$_img"
fi

for p in "$DRIVER_APP" "$SPACESD_APP"; do
  [ -e "$p" ] || { echo "FATAL: missing signed artifact: $p" >&2; exit 1; }
done

# --- create the VM (CI-specific: the hand-run path starts from a live VM) ----
echo "==> Cloning $BASE_IMAGE -> $GOLDEN"
lume delete "$GOLDEN" --force >/dev/null 2>&1 || true
lume clone "$BASE_IMAGE" "$GOLDEN"
# Never `lume run --no-display`; drive it through the API so no window opens on
# the runner's desktop.
curl -s -X POST "$LUME_API/lume/vms/$GOLDEN/run" -H 'content-type: application/json' \
  -d '{"noDisplay":true,"vnc":"enabled"}' --max-time 10 >/dev/null

echo "==> Waiting for SSH"
for _ in $(seq 1 60); do lume ssh "$GOLDEN" "echo ok" 2>/dev/null | grep -q ok && break; sleep 5; done
lume ssh "$GOLDEN" "echo ok" 2>/dev/null | grep -q ok || { echo "FATAL: $GOLDEN never became reachable over ssh" >&2; exit 1; }
IP="$(vmip)"; echo "    guest IP: $IP"

# --- stage CI's signed artifacts where the golden build looks for them -------
# build-golden.sh takes the two daemon bundles from $BUNDLE_DIR (by bundle
# name). In CI they arrive as download-artifact output
# under whatever paths the workflow chose, so give it one directory with the
# names it expects.
#
# Real copies, not symlinks: build-golden.sh ships these with `scp -r`, and the
# SFTP-backed scp on current macOS does not reliably descend a symlink to a
# directory -- which would stage an empty .app and fail an hour later inside the
# guest instead of here. `ditto` also keeps the bundle's extended attributes and
# signature intact, which `cp -R` is not guaranteed to.
STAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cua-golden-stage.XXXXXX")"
trap 'rm -rf "$STAGE_DIR"' EXIT
ditto "$DRIVER_APP" "$STAGE_DIR/CuaDriverLocal.app"
ditto "$SPACESD_APP" "$STAGE_DIR/Cua Spacesd.app"
# A staged copy must still be certificate-backed; an ad-hoc signature does not
# hold the TCC grants the golden seeds, and the failure is silent until a clone
# prompts for Accessibility.
for b in "$STAGE_DIR/CuaDriverLocal.app" "$STAGE_DIR/Cua Spacesd.app"; do
  codesign -dv --verbose=2 "$b" 2>&1 | grep -q "TeamIdentifier=" \
    || { echo "FATAL: $b is not certificate-backed (ad-hoc signatures do not hold seeded TCC grants)" >&2; exit 1; }
done

# --- the whole provisioning phase, from the ONE definition of a golden -------
# Everything a golden contains, in order, ending with the sanitizer and the
# content-verification gate. This is the same script a maintainer runs by hand,
# so the CI image and the hand-built image cannot be different things.
echo "==> Provisioning via golden/build-golden.sh (the single golden definition)"
BUNDLE_DIR="$STAGE_DIR" \
CUA_SUDO_PW="$LUME_SSH_PASSWORD" \
  bash "$GOLDEN_SRC/build-golden.sh" "$GOLDEN"

# build-golden.sh stops the VM, fails loudly if it does not stop, and pins the
# compute spec. From here on it is a frozen image.

if [ -n "$PUSH_IMAGE" ]; then
  echo "==> Pushing $GOLDEN -> $PUSH_REGISTRY/$PUSH_ORG/$PUSH_IMAGE"
  # NOT --single-layer. That mode writes the ENTIRE uncompressed disk to one
  # temporary file before it uploads anything -- ~30 GB of scratch for a 43 GB
  # golden -- and a runner that has just finished building a VM is exactly the
  # machine with no room left. The default OCI path streams 512 MB chunks, so
  # its peak scratch is one chunk, and `lume pull` reads both formats.
  # --single-layer is only needed for kubelet/containerDisk consumers; keep it
  # reachable behind PUSH_SINGLE_LAYER=1 for that case, but never by default.
  _push_args=(--registry "$PUSH_REGISTRY" --organization "$PUSH_ORG")
  if [ "${PUSH_SINGLE_LAYER:-0}" = "1" ]; then
    _push_args+=(--single-layer)
  else
    _push_args+=(--chunk-size-mb "${PUSH_CHUNK_SIZE_MB:-512}")
  fi
  df -h "$HOME" | tail -1
  lume push "$GOLDEN" "$PUSH_IMAGE" "${_push_args[@]}"
else
  echo "==> Build-only (PUSH_IMAGE unset); golden '$GOLDEN' is frozen and ready to clone."
fi
echo "==> Done."
