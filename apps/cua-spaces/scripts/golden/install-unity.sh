#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# install-unity.sh — install Unity Hub and the Editor the demo project targets,
# with no GUI interaction.
#
# Pinned to the version biome-tiles was authored with. The Editor is installed
# through the Hub's headless CLI, which needs BOTH the version and its
# changeset — Unity's download URLs are changeset-addressed, and a version
# alone will not resolve.
#
# Nothing here signs in. The Unity account, its license, and the Unity Version
# Control credentials all arrive later by teleport; an image that bakes them is
# not publishable (see sanitize-golden.sh).
set -uo pipefail

UNITY_VERSION="6000.3.9f1"
# Unity's editor downloads are changeset-addressed, so the version alone will
# not resolve. Verify a change here against
#   https://download.unity3d.com/download_unity/<changeset>/MacEditorInstallerArm64/Unity-<version>.pkg
UNITY_CHANGESET="7a9955a4f2fa"

# Unity publishes per-architecture Hub images; the unsuffixed UnityHubSetup.dmg
# that most guides cite now 404s.
case "$(uname -m)" in
  arm64) HUB_DMG_URL="https://public-cdn.cloud.unity3d.com/hub/prod/UnityHubSetup-arm64.dmg" ;;
  *)     HUB_DMG_URL="https://public-cdn.cloud.unity3d.com/hub/prod/UnityHubSetup-x64.dmg" ;;
esac

HUB_APP="/Applications/Unity Hub.app"
HUB_BIN="${HUB_APP}/Contents/MacOS/Unity Hub"
EDITOR_DIR="/Applications/Unity/Hub/Editor/${UNITY_VERSION}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

say() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }

# sudo cannot prompt over SSH; feed the password in.
sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }

install_rosetta() {
  # Unity refuses to launch without Rosetta 2 — including `-batchmode` — even
  # though the Editor binary is arm64, because it shells out to x86_64 helper
  # tools. On a fresh image this presents as Unity simply never starting.
  if /usr/bin/pgrep -q oahd || [ -d /Library/Apple/usr/share/rosetta ]; then
    say "Rosetta 2 already installed"; return 0
  fi
  say "Installing Rosetta 2 (required by Unity's x86_64 helper tools)"
  sudo_run /usr/sbin/softwareupdate --install-rosetta --agree-to-license \
    || { echo "Rosetta install failed — Unity will not launch without it" >&2; exit 1; }
}

accept_editor_terms() {
  # The Editor blocks on a modal "Unity Software Terms" sheet before it imports
  # anything. It is invisible to a port poll — the main thread parks in
  # SoftwareTermsWindow::EnsureTheUserHasAcceptedSoftwareTerms() — so an agent
  # sees only silence for as long as it sits there. Pre-record the acceptance.
  #
  # The key is Terms-<md5 of the terms version>, so it is tied to a specific
  # terms revision: if Unity publishes new terms the dialog returns and this
  # hash must be refreshed (read it back off a VM where you accepted once, via
  # `plutil -p ~/Library/Preferences/com.unity3d.UnityEditor5.x.plist`).
  local key="Terms-43dc734cac67bb5fb31d4692eb48c6b3"
  defaults write com.unity3d.UnityEditor5.x "$key" -int 1 2>/dev/null \
    && say "Pre-accepted Editor software terms ($key)" \
    || echo "  (could not write editor terms key)" >&2
}

install_hub() {
  if [ -d "$HUB_APP" ]; then
    say "Unity Hub already installed"; return 0
  fi
  say "Downloading Unity Hub"
  curl -fL --retry 3 --retry-delay 2 -o "$WORK/UnityHub.dmg" "$HUB_DMG_URL"

  say "Mounting and copying (non-interactive)"
  # -nobrowse: no Finder window, so nothing needs dismissing.
  # Do not pipe `yes` into hdiutil: it dies on SIGPIPE when hdiutil exits, which
  # under `set -o pipefail` fails the build (exit 141) despite a good mount.
  local attach mnt
  attach="$(hdiutil attach -nobrowse -noverify -noautoopen "$WORK/UnityHub.dmg" 2>&1)" || {
    echo "hdiutil attach failed:" >&2; echo "${attach}" >&2; exit 1; }
  mnt="$(printf '%s\n' "${attach}" | grep -Eo '/Volumes/[^"]+' | tail -1)"
  [ -n "$mnt" ] || { echo "Failed to mount Unity Hub DMG" >&2; exit 1; }
  cp -R "$mnt/Unity Hub.app" /Applications/
  hdiutil detach "$mnt" -quiet || hdiutil detach "$mnt" -force
  # Strip quarantine so launching never raises an "unidentified developer" prompt.
  xattr -dr com.apple.quarantine "$HUB_APP" 2>/dev/null || true
  say "Unity Hub installed"
}

accept_hub_terms() {
  # The Hub refuses headless work until its licence terms are recorded. Writing
  # the marker files avoids the first-run wizard entirely; these carry no
  # identity, so the sanitizer deliberately keeps them.
  local support="$HOME/Library/Application Support/UnityHub"
  mkdir -p "$support"
  # These are bare JSON booleans, not objects: an earlier revision wrote
  # {"seen":true} into firstTimeOpenKey.json, which the Hub reads as falsy.
  printf 'true' > "$support/firstTimeOpenKey.json"
  printf 'true' > "$support/hideGetSetUp.json"
  printf 'true' > "$support/hideHubOnEditorOpen.json"
  # Suppress the Hub's onboarding and licence-provisioning modals. Without
  # showLicenseProvisioning:false the Hub parks a "Get Unity Personal" dialog
  # over itself on first launch, which an agent then has to notice and click —
  # exactly the kind of thing that must not appear in a recorded demo. These
  # values were read back off a VM that had been through the flow manually.
  cat > "$support/firstTimeSettings.json" <<'JSON'
{"showLicenseProvisioning":false,"showEditorRecommendation":false,"showWelcomeModal":false,"showLearnTemplates":false,"showPersonalLicenseEulaFirstTimeModal":false,"hasSeenGetSetUp":true,"hubLicenseEulaAcceptedAt":"2026-06-22T00:00:00Z","hasSeenUnityCliAnnouncement":true}
JSON
  say "Recorded Hub first-run markers + suppressed onboarding modals"
  # These markers do NOT survive into the image on their own. The Hub runs
  # during this build (the Editor is installed through its CLI) and rewrites
  # every one of them on exit — a golden inspected after a build has
  # hideGetSetUp.json=false, firstTimeOpenKey.json=false and
  # hasSeenUnityCliAnnouncement=false again, which is why a Space greeted the
  # agent with "Unity CLI is now available!" over the "Get set up" pane.
  # prepare-space.sh rewrites them on every boot; this build-time copy only
  # keeps the build itself quiet.
}

install_editor() {
  if [ -x "$EDITOR_DIR/Unity.app/Contents/MacOS/Unity" ]; then
    say "Unity ${UNITY_VERSION} already installed"; return 0
  fi
  [ -x "$HUB_BIN" ] || { echo "Unity Hub binary missing" >&2; exit 1; }

  say "Installing Unity ${UNITY_VERSION} (${UNITY_CHANGESET}) via the Hub CLI"
  # `-- --headless` is the Hub's documented CLI entry point. This downloads
  # several GB and takes a while; it prints progress to stdout.
  #
  # --architecture is REQUIRED for an unattended run: without it the Hub stops
  # on an interactive "Please select preferred architecture" picker and waits
  # forever. `</dev/null` is belt-and-braces against any other prompt.
  local arch; case "$(uname -m)" in arm64) arch="arm64";; *) arch="x86_64";; esac
  "$HUB_BIN" -- --headless install \
      --version "$UNITY_VERSION" \
      --changeset "$UNITY_CHANGESET" \
      --architecture "$arch" \
      --module mac-il2cpp --childModules </dev/null || true

  [ -x "$EDITOR_DIR/Unity.app/Contents/MacOS/Unity" ] || {
    echo "Editor not found at $EDITOR_DIR after install." >&2
    echo "Check the changeset matches $UNITY_VERSION at" >&2
    echo "  https://unity.com/releases/editor/archive" >&2
    exit 1
  }
  say "Unity ${UNITY_VERSION} installed"
}

install_rosetta
install_hub
accept_hub_terms
install_editor
accept_editor_terms

say "Versions"
"$EDITOR_DIR/Unity.app/Contents/MacOS/Unity" -version 2>/dev/null | head -1 || true
echo "  Hub:    $(defaults read "$HUB_APP/Contents/Info" CFBundleShortVersionString 2>/dev/null)"
echo "  Editor: $EDITOR_DIR"
