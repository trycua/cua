#!/usr/bin/env bash
# Fail closed unless this is a fresh, manually dispatched GitHub-hosted GUI runner.
set -euo pipefail

ARTIFACT_DIR="${CUA_MACOS_HOSTED_PROBE_DIR:?CUA_MACOS_HOSTED_PROBE_DIR is required}"
SOURCE_SHA="${CUA_E2E_SOURCE_SHA:?CUA_E2E_SOURCE_SHA is required}"
mkdir -p "${ARTIFACT_DIR}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

PROBE_STATUS=failed
PROBE_MESSAGE="probe did not complete"
SWIFT_RESULT="${ARTIFACT_DIR}/window-capture.json"
SYSTEM_LOG="${ARTIFACT_DIR}/system.txt"
WINDOW_METADATA="${ARTIFACT_DIR}/window.json"
PROBE_BINARY="${RUNNER_TEMP:?RUNNER_TEMP is required}/verify-hosted-window"

write_environment() {
  PROBE_STATUS="${PROBE_STATUS}" \
  PROBE_MESSAGE="${PROBE_MESSAGE}" \
  SWIFT_RESULT="${SWIFT_RESULT}" \
  SYSTEM_LOG="${SYSTEM_LOG}" \
  python3 - "${ARTIFACT_DIR}/environment.json" <<'PY'
import json
import os
from pathlib import Path
import platform

swift_result = Path(os.environ["SWIFT_RESULT"])
payload = {
    "schema": "cua-driver/macos-hosted-probe@v1",
    "status": os.environ["PROBE_STATUS"],
    "message": os.environ["PROBE_MESSAGE"],
    "source_sha": os.environ.get("CUA_E2E_SOURCE_SHA", ""),
    "github": {
        "actions": os.environ.get("GITHUB_ACTIONS", ""),
        "event_name": os.environ.get("GITHUB_EVENT_NAME", ""),
        "run_id": os.environ.get("GITHUB_RUN_ID", ""),
        "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT", ""),
        "runner_environment": os.environ.get("RUNNER_ENVIRONMENT", ""),
        "runner_image": os.environ.get("ImageOS", ""),
        "runner_image_version": os.environ.get("ImageVersion", ""),
    },
    "system": {
        "architecture": platform.machine(),
        "macos_version": platform.mac_ver()[0],
        "system_log": os.environ["SYSTEM_LOG"],
    },
}
if swift_result.is_file():
    try:
        payload["window_capture"] = json.loads(swift_result.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        payload["window_capture_error"] = str(exc)
Path(__import__("sys").argv[1]).write_text(json.dumps(payload, indent=2) + "\n")
PY
}
trap write_environment EXIT
write_environment

fail() {
  PROBE_MESSAGE="$1"
  echo "${PROBE_MESSAGE}" >&2
  exit 1
}

run_with_deadline() {
  local seconds="$1"
  shift
  "$@" &
  local command_pid=$!
  (
    sleep "${seconds}"
    kill -TERM "${command_pid}" 2>/dev/null || true
  ) &
  local watchdog_pid=$!
  local command_status=0
  set +e
  wait "${command_pid}"
  command_status=$?
  set -e
  kill "${watchdog_pid}" 2>/dev/null || true
  wait "${watchdog_pid}" 2>/dev/null || true
  if [[ "${command_status}" == 143 ]]; then
    return 124
  fi
  return "${command_status}"
}

[[ "${GITHUB_ACTIONS:-}" == true ]] || fail "GITHUB_ACTIONS must be true"
[[ "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail "runner must be GitHub-hosted"
[[ "${CI:-}" == true ]] || fail "CI must be true"
[[ "${GITHUB_EVENT_NAME:-}" == workflow_dispatch ]] || fail "probe must be manually dispatched"
[[ "${SOURCE_SHA}" =~ ^[0-9a-fA-F]{40}$ ]] || fail "source SHA must contain 40 hexadecimal characters"
[[ -z "${SSH_CONNECTION:-}${SSH_CLIENT:-}${SSH_TTY:-}" ]] || fail "SSH sessions cannot seed or certify hosted TCC state"

CURRENT_USER="$(id -un)"
CONSOLE_USER="$(stat -f '%Su' /dev/console)"
[[ "${CURRENT_USER}" == runner ]] || fail "current user must be runner, got ${CURRENT_USER}"
[[ "${CONSOLE_USER}" == runner ]] || fail "console user must be runner, got ${CONSOLE_USER}"

CURRENT_UID="$(id -u)"
launchctl print "gui/${CURRENT_UID}" >/dev/null 2>&1 || fail "Aqua gui/${CURRENT_UID} launch domain is unavailable"
pgrep -x WindowServer >/dev/null || fail "WindowServer is unavailable"

SIP_STATUS="$(csrutil status 2>&1 || true)"

{
  echo "source_sha=${SOURCE_SHA}"
  echo "current_user=${CURRENT_USER}"
  echo "console_user=${CONSOLE_USER}"
  echo "uid=${CURRENT_UID}"
  echo "architecture=$(uname -m)"
  echo "kernel=$(uname -a)"
  echo "macos_version=$(sw_vers -productVersion)"
  echo "macos_build=$(sw_vers -buildVersion)"
  echo "sip_status=${SIP_STATUS}"
  echo "runner_image=${ImageOS:-unknown}"
  echo "runner_image_version=${ImageVersion:-unknown}"
  echo "memory_bytes=$(sysctl -n hw.memsize)"
  echo "logical_cpus=$(sysctl -n hw.logicalcpu)"
  df -h /
  system_profiler SPDisplaysDataType -json
  launchctl print "gui/${CURRENT_UID}" | sed -n '1,20p'
  pgrep -lf WindowServer || true
} > "${SYSTEM_LOG}"
write_environment

[[ "${SIP_STATUS}" == *"System Integrity Protection status: disabled"* ]] \
  || fail "System Integrity Protection must be disabled on the disposable hosted runner: ${SIP_STATUS}"

MARKER="CUA HOSTED MACOS GUI PROBE"
DOCUMENT="${ARTIFACT_DIR}/probe.txt"
printf '%s\n\n%s\n\n%s\n' "${MARKER}" "${MARKER}" "${MARKER}" > "${DOCUMENT}"

open -a TextEdit "${DOCUMENT}"
for _ in {1..30}; do
  FRONTMOST="$(osascript -e 'tell application "System Events" to tell process "TextEdit" to get frontmost' 2>/dev/null || true)"
  if [[ "${FRONTMOST}" == true ]]; then
    break
  fi
  sleep 1
done
[[ "${FRONTMOST:-}" == true ]] || fail "TextEdit did not become frontmost"

if ! run_with_deadline 30 osascript > "${ARTIFACT_DIR}/textedit-automation.log" 2>&1 <<'APPLESCRIPT'
tell application "TextEdit" to activate
tell application "Finder" to set desktopBounds to bounds of window of desktop
set targetWidth to (item 3 of desktopBounds) - 120
set targetHeight to (item 4 of desktopBounds) - 140
if targetWidth > 800 then set targetWidth to 800
if targetHeight > 600 then set targetHeight to 600
tell application "System Events"
  tell process "TextEdit"
    set frontmost to true
    set position of front window to {60, 80}
    set size of front window to {targetWidth, targetHeight}
  end tell
end tell
APPLESCRIPT
then
  fail "TextEdit window automation failed or timed out"
fi

if ! run_with_deadline 120 xcrun swiftc \
  "${REPO_ROOT}/scripts/ci/macos/verify-hosted-window.swift" \
  -o "${PROBE_BINARY}" \
  > "${ARTIFACT_DIR}/swift-build.log" 2>&1
then
  fail "hosted capture verifier compilation failed or timed out"
fi

if ! run_with_deadline 30 "${PROBE_BINARY}" --locate \
  > "${WINDOW_METADATA}" 2> "${ARTIFACT_DIR}/window-locate.stderr.log"
then
  fail "TextEdit window discovery failed or timed out"
fi
WINDOW_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "${WINDOW_METADATA}")"

if ! run_with_deadline 30 /usr/sbin/screencapture -x -l "${WINDOW_ID}" \
  "${ARTIFACT_DIR}/textedit-window.png" \
  > "${ARTIFACT_DIR}/window-screencapture.log" 2>&1
then
  fail "window-scoped screencapture failed or timed out"
fi
if ! run_with_deadline 30 /usr/sbin/screencapture -x \
  "${ARTIFACT_DIR}/display.png" \
  > "${ARTIFACT_DIR}/display-screencapture.log" 2>&1
then
  fail "display screencapture failed or timed out"
fi

if ! run_with_deadline 120 "${PROBE_BINARY}" --verify \
  --marker "${MARKER}" \
  --window-input "${ARTIFACT_DIR}/textedit-window.png" \
  --display-input "${ARTIFACT_DIR}/display.png" \
  --window-metadata "${WINDOW_METADATA}" \
  > "${SWIFT_RESULT}" 2> "${ARTIFACT_DIR}/window-capture.stderr.log"
then
  fail "window/display OCR verification failed or timed out"
fi

python3 - "${SWIFT_RESULT}" <<'PY'
import json
import sys
from pathlib import Path

result = json.loads(Path(sys.argv[1]).read_text())
required = (
    result.get("accessibility_trusted") is True,
    result.get("screen_capture_preflight") is True,
    result.get("marker_recognized") is True,
    result.get("display_capture", {}).get("marker_recognized") is True,
    result.get("window", {}).get("owner") == "TextEdit",
    result.get("window", {}).get("name") == "probe.txt",
    result.get("window", {}).get("width", 0) >= 400,
    result.get("window", {}).get("height", 0) >= 250,
)
if not all(required):
    raise SystemExit(f"hosted GUI probe failed: {result}")
PY

if ! run_with_deadline 30 osascript \
  -e 'tell application "TextEdit" to close every window saving no' \
  -e 'tell application "TextEdit" to quit'; then
  fail "TextEdit probe cleanup failed or timed out"
fi

PROBE_STATUS=passed
PROBE_MESSAGE="hosted macOS GUI environment and TextEdit window capture passed"
echo "${PROBE_MESSAGE}"
