#!/usr/bin/env bash
# Install and run the canonical macOS GUI matrix inside a disposable Lume clone.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../../.." && pwd)"
DRIVER_ROOT="${REPO_ROOT}/libs/cua-driver"
RUST_ROOT="${DRIVER_ROOT}/rust"
ARTIFACT_DIR="${REPO_ROOT}/artifacts/cua-driver/macos"
SOURCE_MARKER="${CUA_E2E_SOURCE_MARKER:-${REPO_ROOT}/.cua-e2e-source-sha}"

usage() {
  cat <<'EOF'
Usage: run-all.sh [--no-build]

Run the canonical cua-driver macOS GUI E2E matrix in a disposable clone of
the maintainer Lume golden image. Start this command from Terminal in the VM's
logged-in desktop session, not over SSH.
EOF
}

for arg in "$@"; do
  case "${arg}" in
    --no-build) ;;
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

for command_name in cargo codesign ffmpeg ffprobe jq node npm xcrun; do
  command -v "${command_name}" >/dev/null 2>&1 || {
    echo "Missing golden-image dependency: ${command_name}" >&2
    exit 2
  }
done

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

codesign -d -r- /Applications/CuaDriver.app \
  > "${ARTIFACT_DIR}/codesign-requirement.txt" 2>&1
if ! grep -Fq "certificate leaf" "${ARTIFACT_DIR}/codesign-requirement.txt"; then
  echo "CuaDriver.app is not signed with the golden image's stable certificate identity" >&2
  exit 1
fi

INSTALLED_BIN="${HOME}/.local/bin/cua-driver"
PERMISSIONS_FILE="${ARTIFACT_DIR}/permissions.json"
PERMISSIONS_READY=0
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if "${INSTALLED_BIN}" permissions status --json > "${PERMISSIONS_FILE}" \
      && jq -e '
        .accessibility == true
        and .screen_recording == true
        and .screen_recording_capturable == true
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

echo "[E2E] Running the canonical macOS matrix"
exec "${REPO_ROOT}/scripts/ci/macos/run-rust-e2e.sh" "$@"
