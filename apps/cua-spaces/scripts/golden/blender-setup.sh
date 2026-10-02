#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

#
# blender-setup.sh — non-interactive Blender + blender-mcp setup.
# Target: macOS 26.x, Apple Silicon (arm64)
#
# Installs (all pinned, no GUI dialogs):
#   - Blender 4.5.13 LTS (macos-arm64) via direct .dmg download -> /Applications
#   - uv / uvx via the official standalone installer (NOT brew)
#   - ahujasid/blender-mcp addon (addon.py) into Blender's user addons dir,
#     enabled headlessly via `blender --background --python-expr`
#   - the blender-mcp server package, pre-warmed into the uv cache
#
# Verified against:
#   https://github.com/ahujasid/blender-mcp  (addon.py, PyPI blender-mcp)
#   https://download.blender.org/release/Blender4.5/
#   https://docs.astral.sh/uv/getting-started/installation/
#
set -euo pipefail

# --------------------------------------------------------------------------
# Pinned versions — change here only
# --------------------------------------------------------------------------
BLENDER_VER="4.5.13"
BLENDER_SERIES="4.5"
BLENDER_DMG="blender-${BLENDER_VER}-macos-arm64.dmg"
BLENDER_MD5="99682cd8ab958436209ac116d9f3b649"
UV_VER="0.12.13"
BLENDER_MCP_PIP="blender-mcp==1.9.1"

# Primary = official host; mirror = fallback if it 403s or egress-blocks.
DL_PRIMARY="https://download.blender.org/release/Blender${BLENDER_SERIES}/${BLENDER_DMG}"
DL_MIRROR="https://mirrors.ocf.berkeley.edu/blender/release/Blender${BLENDER_SERIES}/${BLENDER_DMG}"

BLENDER_APP="/Applications/Blender.app"
BLENDER_BIN="${BLENDER_APP}/Contents/MacOS/Blender"
ADDONS_DIR="${HOME}/Library/Application Support/Blender/${BLENDER_SERIES}/scripts/addons"
WORK="$(mktemp -d)"
REPO_DIR="${WORK}/blender-mcp"

log(){ printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
cleanup(){ rm -rf "${WORK}" 2>/dev/null || true; }
trap cleanup EXIT

install_blender(){
  if [ -x "${BLENDER_BIN}" ]; then
    have="$("${BLENDER_BIN}" --version 2>/dev/null | head -1 || true)"
    if printf '%s' "${have}" | grep -q "${BLENDER_VER}"; then
      log "Blender ${BLENDER_VER} already installed: ${have}"; return 0
    fi
    log "Different Blender present (${have}); replacing with ${BLENDER_VER}"
    rm -rf "${BLENDER_APP}"
  fi

  local dmg="${WORK}/${BLENDER_DMG}"
  log "Downloading ${BLENDER_DMG}"
  if ! curl -fL --retry 3 --retry-delay 2 -o "${dmg}" "${DL_PRIMARY}"; then
    log "Primary host failed, using mirror"
    curl -fL --retry 3 --retry-delay 2 -o "${dmg}" "${DL_MIRROR}"
  fi

  log "Verifying MD5"
  got="$(md5 -q "${dmg}")"
  if [ "${got}" != "${BLENDER_MD5}" ]; then
    echo "MD5 mismatch: expected ${BLENDER_MD5}, got ${got}" >&2; exit 1
  fi

  log "Mounting DMG (non-interactive) and copying Blender.app -> /Applications"
  # NOTE: do not pipe `yes` into hdiutil. When hdiutil exits, `yes` is killed by
  # SIGPIPE, and under `set -o pipefail` that surfaces as exit 141 and fails the
  # whole build even though the mount succeeded. These images carry no EULA
  # prompt, so attach directly and parse the output afterwards.
  local attach mnt
  attach="$(hdiutil attach -nobrowse -noverify -noautoopen "${dmg}" 2>&1)" || {
    echo "hdiutil attach failed:" >&2; echo "${attach}" >&2; exit 1; }
  mnt="$(printf '%s\n' "${attach}" | grep -Eo '/Volumes/[^"]+' | tail -1)"
  [ -n "${mnt}" ] || { echo "Failed to mount DMG" >&2; exit 1; }
  cp -R "${mnt}/Blender.app" /Applications/
  hdiutil detach "${mnt}" -quiet || hdiutil detach "${mnt}" -force

  # Strip Gatekeeper quarantine so Blender runs without an "unidentified
  # developer" prompt — unanswerable inside a Space.
  xattr -dr com.apple.quarantine "${BLENDER_APP}" 2>/dev/null || true

  [ -x "${BLENDER_BIN}" ] || { echo "Blender binary missing after copy" >&2; exit 1; }
  log "Installed: $("${BLENDER_BIN}" --version | head -1)"
}

install_uv(){
  export PATH="${HOME}/.local/bin:${PATH}"
  if command -v uvx >/dev/null 2>&1 && uv --version 2>/dev/null | grep -q "${UV_VER}"; then
    log "uv ${UV_VER} already present"; return 0
  fi
  log "Installing uv ${UV_VER} (pinned standalone installer)"
  curl -LsSf "https://astral.sh/uv/${UV_VER}/install.sh" | env INSTALLER_NO_MODIFY_PATH=1 sh
  hash -r 2>/dev/null || true
  command -v uvx >/dev/null 2>&1 || { echo "uvx not on PATH after install" >&2; exit 1; }
  log "uv installed: $(uv --version)"
}

install_addon(){
  log "Cloning ahujasid/blender-mcp"
  git clone --depth 1 https://github.com/ahujasid/blender-mcp.git "${REPO_DIR}"

  local src="${REPO_DIR}/addon.py"
  [ -f "${src}" ] || { echo "addon.py not found in repo root" >&2; exit 1; }

  # Install as module name 'blender_mcp' (file stem == addon_enable module id)
  mkdir -p "${ADDONS_DIR}"
  cp "${src}" "${ADDONS_DIR}/blender_mcp.py"
  log "Copied addon.py -> ${ADDONS_DIR}/blender_mcp.py"

  log "Enabling addon (pass 1: from factory settings, non-interactive)"
  "${BLENDER_BIN}" --background -noaudio --factory-startup --python-exit-code 1 --python-expr \
'import bpy
try:
    bpy.ops.preferences.addon_enable(module="blender_mcp")
    bpy.ops.wm.save_userpref()
    print("BLENDER_MCP_ADDON_ENABLED_OK")
except Exception as e:
    print("BLENDER_MCP_ADDON_ENABLE_FAILED:", e); raise'

  # Pass 2, WITHOUT --factory-startup, for the preferences.
  #
  # Blender's splash is a borderless window that takes focus on launch and is
  # never dismissed by anything an agent does; it was on screen for a whole
  # demo run. It used to be turned off in pass 1 above, in the same
  # save_userpref call that enables the addon — and that DOES NOT STICK. Read
  # back from a later process, `show_splash` was still True while the addon
  # enable from the very same write had persisted. A --factory-startup process
  # saves its prefs from factory defaults, and show_splash comes back as one of
  # them. Setting it in a normal (non-factory) session, which loads the file
  # written by pass 1 and rewrites it, does stick.
  log "Suppressing splash + developer UI (pass 2: on the saved preferences)"
  "${BLENDER_BIN}" --background -noaudio --python-exit-code 1 --python-expr \
'import bpy
p = bpy.context.preferences
p.view.show_splash = False
p.view.show_developer_ui = False
bpy.ops.wm.save_userpref()
print("BLENDER_PREFS_SAVED splash=%s" % p.view.show_splash)'

  # Verify in a THIRD process: the only proof that a preference persisted is
  # reading it back from a process that did not write it.
  log "Verifying the saved preferences"
  "${BLENDER_BIN}" --background -noaudio --python-exit-code 1 --python-expr \
'import addon_utils, bpy, sys
enabled = addon_utils.check("blender_mcp")[1]
splash = bpy.context.preferences.view.show_splash
print("BLENDER_VERIFY addon_enabled=%s show_splash=%s" % (enabled, splash))
if not enabled or splash:
    sys.exit(1)'

  log "Addon 'MCP for Blender' installed and enabled; splash suppressed"
}

prime_server(){
  export PATH="${HOME}/.local/bin:${PATH}"
  log "Pre-fetching ${BLENDER_MCP_PIP} into the uv cache"
  uvx --from "${BLENDER_MCP_PIP}" blender-mcp --help >/dev/null 2>&1 || true
  log "blender-mcp server cached"
}

main(){
  install_blender
  install_uv
  install_addon
  prime_server

  cat <<EOF

============================================================================
 DONE. Blender ${BLENDER_VER} + blender-mcp installed (Apple Silicon).
============================================================================

STARTING THE SOCKET SERVER (port 9876)
--------------------------------------
Nothing here starts it, and nothing needs to be started by hand: prepare-space.sh
launches Blender at every boot with

  "${BLENDER_BIN}" -noaudio --python ~/.cua/blender-mcp-start.py

and then WAITS for 127.0.0.1:9876 to accept a connection before declaring the
Space ready. This block used to print that command for a human to run, and
because no human ever did, every fresh Space came up with Blender not running
and nothing on 9876 while the skill text claimed otherwise.

  * NOT --background: the server relies on Blender's modal event loop, which
    does not run in background mode.
  * -noaudio: CoreAudio initialisation hangs in this VM without it.
  * HOME must be correct for the process, or Blender cannot find the addon.

THE MCP SERVER (what the agent talks to)
----------------------------------------
Register \`uvx blender-mcp\` with the agent (see the golden's
write-agent-mcp.sh); do not run it by hand in a terminal.
============================================================================
EOF
}

main "$@"
