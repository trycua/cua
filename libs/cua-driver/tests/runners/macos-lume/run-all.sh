#!/usr/bin/env bash
# Install and run the canonical macOS GUI matrix inside a disposable Lume clone.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../../.." && pwd)"
DRIVER_ROOT="${REPO_ROOT}/libs/cua-driver"
RUST_ROOT="${DRIVER_ROOT}/rust"
ARTIFACT_DIR="${REPO_ROOT}/artifacts/cua-driver/macos"
SOURCE_MARKER="${CUA_E2E_SOURCE_MARKER:-${REPO_ROOT}/.cua-e2e-source-sha}"
SIGNING_KEYCHAIN="${CUA_E2E_SIGNING_KEYCHAIN:-${HOME}/Library/Keychains/cua-driver-signing.keychain-db}"
SIGNING_CN="CuaDriver Local Signing (cua-driver-rs)"

usage() {
  cat <<'EOF'
Usage: run-all.sh [--no-build] [--standalone-browser]

Run the canonical cua-driver macOS GUI E2E matrix in a disposable clone of
the maintainer Lume golden image. Start this command from Terminal in the VM's
logged-in desktop session, not over SSH.

--standalone-browser also runs the optional installed Chrome/Edge browser-tool
matrix after the canonical repo-local harness matrix.
EOF
}

RUN_STANDALONE_BROWSER=0
NO_BUILD=0
for arg in "$@"; do
  case "${arg}" in
    --no-build) NO_BUILD=1 ;;
    --standalone-browser) RUN_STANDALONE_BROWSER=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: ${arg}" >&2; usage >&2; exit 2 ;;
  esac
done

[[ "$(uname -s)" == Darwin ]] || {
  echo "The Lume macOS runner must run in a macOS guest" >&2
  exit 2
}
if [[ -n "${SSH_CONNECTION:-}" || -n "${SSH_TTY:-}" ]]; then
  echo "Run this command from Terminal in the VM display so fixtures inherit the GUI login session" >&2
  exit 2
fi

CURRENT_USER="$(id -un)"
CONSOLE_USER="$(stat -f '%Su' /dev/console)"
if [[ "${CONSOLE_USER}" != "${CURRENT_USER}" ]]; then
  echo "The current user (${CURRENT_USER}) is not the logged-in console user (${CONSOLE_USER})" >&2
  exit 2
fi
launchctl print "gui/$(id -u)" >/dev/null 2>&1 || {
  echo "No launchd GUI domain is available for ${CURRENT_USER}" >&2
  exit 2
}

SIP_STATUS="$(csrutil status 2>&1)"
if ! printf '%s\n' "${SIP_STATUS}" | grep -Fq "System Integrity Protection status: disabled."; then
  printf '%s\n' "${SIP_STATUS}" >&2
  echo "The canonical Lume behavior lane requires the SIP-off golden image" >&2
  exit 2
fi

for command_name in cargo codesign ffmpeg ffprobe jq node npm osascript security xcrun; do
  command -v "${command_name}" >/dev/null 2>&1 || {
    echo "Missing golden-image dependency: ${command_name}" >&2
    exit 2
  }
done

if [[ ! -f "${SIGNING_KEYCHAIN}" ]]; then
  echo "Missing golden-image signing keychain: ${SIGNING_KEYCHAIN}" >&2
  echo "Create the private seed according to tests/runners/macos-lume/README.md" >&2
  exit 2
fi
echo "[SIGNING] Unlocking the golden image's dedicated signing keychain"
if [[ -n "${CUA_E2E_SIGNING_KEYCHAIN_PASSWORD:-}" ]]; then
  security unlock-keychain -p "${CUA_E2E_SIGNING_KEYCHAIN_PASSWORD}" "${SIGNING_KEYCHAIN}"
  unset CUA_E2E_SIGNING_KEYCHAIN_PASSWORD
else
  security unlock-keychain "${SIGNING_KEYCHAIN}"
fi
if ! security find-identity -v -p codesigning "${SIGNING_KEYCHAIN}" 2>/dev/null \
    | grep -Fq "\"${SIGNING_CN}\""; then
  echo "The dedicated keychain has no valid ${SIGNING_CN} identity" >&2
  exit 2
fi
export CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN="${SIGNING_KEYCHAIN}"

if git -C "${REPO_ROOT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if [[ -n "$(git -C "${REPO_ROOT}" status --porcelain --untracked-files=normal)" ]]; then
    echo "The synced source must have no working-tree changes" >&2
    exit 2
  fi
  SOURCE_SHA="$(git -C "${REPO_ROOT}" rev-parse HEAD)"
elif [[ -f "${SOURCE_MARKER}" ]]; then
  SOURCE_SHA="$(tr -d '[:space:]' < "${SOURCE_MARKER}")"
  export CUA_E2E_SOURCE_MARKER="${SOURCE_MARKER}"
else
  echo "The synced source needs git metadata or ${SOURCE_MARKER}" >&2
  exit 2
fi
if [[ ! "${SOURCE_SHA}" =~ ^[0-9a-fA-F]{40}$ ]]; then
  echo "The synced source identity is not a full commit SHA: ${SOURCE_SHA}" >&2
  exit 2
fi

export CUA_E2E_SOURCE_SHA="${SOURCE_SHA}"
export CUA_DRIVER_SOURCE_SHA="${SOURCE_SHA}"
mkdir -p "${ARTIFACT_DIR}"
printf '%s\n' "${SIP_STATUS}" > "${ARTIFACT_DIR}/sip-status.txt"
printf '%s\n' "${SOURCE_SHA}" > "${ARTIFACT_DIR}/requested-source-sha.txt"
{
  sw_vers
  printf 'console_user:\t%s\n' "${CONSOLE_USER}"
  printf 'rustc:\t%s\n' "$(rustc --version)"
  printf 'node:\t%s\n' "$(node --version)"
  printf 'xcode_select:\t%s\n' "$(xcode-select -p)"
} > "${ARTIFACT_DIR}/golden-environment.txt"

echo "[AUTOMATION] Verifying Terminal can read frontmost state through System Events"
if ! osascript -e \
    'tell application "System Events" to get name of first application process whose frontmost is true' \
    > "${ARTIFACT_DIR}/terminal-system-events.txt"; then
  echo "Terminal cannot control System Events; rebuild the seed and grant the Automation prompt" >&2
  exit 2
fi

# CUA_DRIVER_SOURCE_SHA is consumed by option_env! in platform-macos. Cargo
# does not treat arbitrary compile-time environment variables as an input when
# deciding whether an otherwise unchanged crate is fresh. Force that crate and
# its dependent driver binary to rebuild so a doc-only commit cannot inherit a
# stale embedded source SHA from the seed or a previous worker run.
echo "[BUILD IDENTITY] Invalidating the macOS backend for ${SOURCE_SHA}"
cargo clean --manifest-path "${RUST_ROOT}/Cargo.toml" -p platform-macos

echo "[INSTALL] Building and installing ${SOURCE_SHA} with the golden signing identity"
bash "${DRIVER_ROOT}/scripts/install-local.sh" --release --autostart \
  2>&1 | tee "${ARTIFACT_DIR}/install-local.log"

codesign -d -r- /Applications/CuaDriverLocal.app \
  > "${ARTIFACT_DIR}/codesign-requirement.txt" 2>&1
if ! grep -Fq "certificate leaf" "${ARTIFACT_DIR}/codesign-requirement.txt"; then
  echo "CuaDriverLocal.app is not signed with the golden image's stable certificate identity" >&2
  exit 1
fi

INSTALLED_BIN="${HOME}/.local/bin/cua-driver-local"
PERMISSIONS_FILE="${ARTIFACT_DIR}/permissions.json"
PERMISSIONS_READY=0
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if "${INSTALLED_BIN}" permissions status --json > "${PERMISSIONS_FILE}" \
      && jq -e '
        .accessibility == true
        and .screen_recording == true
        and .screen_recording_capturable == null
        and .direct_capture_status == "not_checked"
        and .source.attribution == "driver-daemon"
      ' "${PERMISSIONS_FILE}" >/dev/null; then
    PERMISSIONS_READY=1
    break
  fi
  sleep 1
done
if [[ "${PERMISSIONS_READY}" != 1 ]]; then
  cat "${PERMISSIONS_FILE}" >&2 || true
  echo "The cloned golden image does not have usable Accessibility and Screen Recording grants" >&2
  exit 1
fi

LIVE_PERMISSIONS_FILE="${ARTIFACT_DIR}/permissions-live-capture.json"
if ! "${INSTALLED_BIN}" call check_permissions '{"prompt":true}' \
    > "${LIVE_PERMISSIONS_FILE}" \
    || ! jq -e '
      .structuredContent.screen_recording_capturable == true
      and .structuredContent.direct_capture_status == "ready"
    ' "${LIVE_PERMISSIONS_FILE}" >/dev/null; then
  cat "${LIVE_PERMISSIONS_FILE}" >&2 || true
  echo "The cloned golden image does not have usable direct ScreenCaptureKit consent" >&2
  exit 1
fi

echo "[APP DISCOVERY] Verifying the installed CuaDriver can enumerate apps"
if ! "${INSTALLED_BIN}" list_apps '{}' > "${ARTIFACT_DIR}/driver-list-apps.json"; then
  echo "CuaDriver cannot enumerate applications" >&2
  exit 2
fi

echo "[E2E] Running the canonical macOS matrix"
if [[ "${NO_BUILD}" == 1 ]]; then
  "${REPO_ROOT}/scripts/ci/macos/run-rust-e2e.sh" --no-build
else
  "${REPO_ROOT}/scripts/ci/macos/run-rust-e2e.sh"
fi

if [[ "${RUN_STANDALONE_BROWSER}" == 1 ]]; then
  echo "[E2E] Running the optional standalone browser matrix"
  BROWSER_ARTIFACT_DIR="${REPO_ROOT}/artifacts/cua-driver/macos-standalone-browser"
  if [[ -d "${BROWSER_ARTIFACT_DIR}" ]] \
      && [[ -n "$(find "${BROWSER_ARTIFACT_DIR}" -mindepth 1 -print -quit)" ]]; then
    BROWSER_ARTIFACT_ARCHIVE="$(mktemp -d "${TMPDIR:-/tmp}/cua-macos-browser-e2e.XXXXXX")"
    mv "${BROWSER_ARTIFACT_DIR}" "${BROWSER_ARTIFACT_ARCHIVE}/macos-standalone-browser"
    echo "Previous standalone-browser evidence preserved at ${BROWSER_ARTIFACT_ARCHIVE}/macos-standalone-browser"
  fi
  CUA_E2E_ARTIFACT_DIR="${BROWSER_ARTIFACT_DIR}" \
    "${REPO_ROOT}/scripts/ci/run-rust-standalone-browser-e2e.sh"
fi
