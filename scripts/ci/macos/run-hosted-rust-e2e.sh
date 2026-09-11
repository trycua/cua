#!/usr/bin/env bash
# Prepare a fresh GitHub-hosted macOS 26 runner and run one canonical E2E lane.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
DRIVER_ROOT="${REPO_ROOT}/libs/cua-driver"
LOCAL_APP="/Applications/CuaDriverLocal.app"
INSTALLED_BIN="${LOCAL_APP}/Contents/MacOS/cua-driver-local"
TCC_SEEDER="${DRIVER_ROOT}/tests/runners/macos-lume/seed-tcc-guest.sh"
LANE="${CUA_E2E_INTERNAL_LANE:-}"
BOOTSTRAP_DIR="${REPO_ROOT}/artifacts/cua-driver/macos-hosted-bootstrap"
KEYCHAIN="${RUNNER_TEMP:-}/cua-driver-hosted-signing.keychain-db"
DAEMON_SOCKET="${HOME}/Library/Caches/cua-driver-local/cua-driver-local.sock"
SCREEN_CAPTURE_APPROVALS="${HOME}/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist"
SCREEN_CAPTURE_CLIENT="com.trycua.driver.local"
KEYCHAIN_PASSWORD=""
DAEMON_STARTED=0
TRUSTED_IDENTITY=""
WATCHDOG_PID=""
RUN_START="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
ORIGINAL_KEYCHAINS=()

run_bounded() {
  local seconds="$1"
  shift
  /usr/bin/perl -e 'alarm shift; exec @ARGV' "${seconds}" "$@"
}

mark_phase() {
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" \
    | tee "${BOOTSTRAP_DIR}/phase.txt"
}

capture_diagnostics() {
  local command_status=$?
  trap - EXIT
  set +e
  mkdir -p "${BOOTSTRAP_DIR}"
  printf '%s\n' "${command_status}" > "${BOOTSTRAP_DIR}/exit-status.txt"
  if [[ -n "${WATCHDOG_PID}" ]]; then
    kill "${WATCHDOG_PID}" >/dev/null 2>&1
    wait "${WATCHDOG_PID}" 2>/dev/null
  fi
  pgrep -lf 'cua-driver-local|tccd|WindowServer' \
    > "${BOOTSTRAP_DIR}/relevant-processes.txt" 2>&1
  launchctl print "gui/$(id -u)" > "${BOOTSTRAP_DIR}/launchd-gui.txt" 2>&1
  if [[ -x "${INSTALLED_BIN}" ]]; then
    "${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" status \
      > "${BOOTSTRAP_DIR}/daemon-status.txt" 2>&1
    "${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" permissions status --json \
      > "${BOOTSTRAP_DIR}/permissions-final.json" 2> "${BOOTSTRAP_DIR}/permissions-final.err"
    codesign -dvvv -r- "${LOCAL_APP}" \
      > "${BOOTSTRAP_DIR}/codesign-final.txt" 2>&1
    if [[ "${DAEMON_STARTED}" == 1 ]]; then
      "${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" stop >/dev/null 2>&1
    fi
  fi
  if [[ "${command_status}" == 0 ]]; then
    /usr/bin/perl -e 'alarm shift; exec @ARGV' 60 \
      /usr/bin/log show --last 15m --style compact \
      --predicate 'process == "cua-driver-local" OR process == "tccd"' \
      > "${BOOTSTRAP_DIR}/unified-log.txt" 2>&1
  else
    /usr/bin/perl -e 'alarm shift; exec @ARGV' 60 \
      /usr/bin/log show --start "${RUN_START}" --style compact \
      --predicate 'process == "cua-driver-local" OR process == "tccd"' \
      > "${BOOTSTRAP_DIR}/unified-log.txt" 2>&1
  fi
  if [[ "${TRUSTED_IDENTITY}" =~ ^[0-9A-Fa-f]{40}$ ]]; then
    run_bounded 20 sudo -n security remove-trusted-cert -d "${CERTIFICATE_PEM}" \
      >/dev/null 2>&1
    run_bounded 20 sudo -n security delete-certificate -Z "${TRUSTED_IDENTITY}" \
      /Library/Keychains/System.keychain >/dev/null 2>&1
  fi
  if ((${#ORIGINAL_KEYCHAINS[@]})); then
    run_bounded 20 security list-keychains -d user -s \
      "${ORIGINAL_KEYCHAINS[@]}" >/dev/null 2>&1
  fi
  if [[ -n "${KEYCHAIN}" && -f "${KEYCHAIN}" ]]; then
    run_bounded 20 security delete-keychain "${KEYCHAIN}" >/dev/null 2>&1
  fi
  exit "${command_status}"
}
trap capture_diagnostics EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail() {
  echo "hosted-macos-e2e: $*" >&2
  exit 2
}

capture_command_output() {
  local output_file="$1"
  shift
  "$@" > "${output_file}" 2>&1
}

[[ "$(uname -s)" == Darwin ]] || fail "requires macOS"
[[ "${GITHUB_ACTIONS:-}" == true ]] || fail "requires GitHub Actions"
[[ "${GITHUB_EVENT_NAME:-}" == workflow_dispatch ]] \
  || fail "runs only from workflow_dispatch"
[[ "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] \
  || fail "requires a GitHub-hosted runner"
[[ "${RUNNER_OS:-}" == macOS ]] || fail "requires RUNNER_OS=macOS"
[[ "${ImageOS:-}" == macos26 ]] || fail "requires the macos-26 runner image"
[[ -z "${SSH_CONNECTION:-}" && -z "${SSH_TTY:-}" ]] \
  || fail "SSH sessions cannot seed or certify hosted TCC state"
case "${LANE}" in
  shared|native|capture) ;;
  *) fail "CUA_E2E_INTERNAL_LANE must be shared, native, or capture" ;;
esac

CURRENT_USER="$(id -un)"
CONSOLE_USER="$(stat -f '%Su' /dev/console)"
[[ "${CURRENT_USER}" == runner && "${CONSOLE_USER}" == runner ]] \
  || fail "expected runner as current and console user; got ${CURRENT_USER}/${CONSOLE_USER}"
launchctl print "gui/$(id -u)" >/dev/null 2>&1 \
  || fail "no Aqua launchd GUI domain is available"

SIP_STATUS="$(csrutil status 2>&1 || true)"
[[ "${SIP_STATUS}" == "System Integrity Protection status: disabled." ]] \
  || fail "requires SIP disabled; got: ${SIP_STATUS}"
MODEL="$(sysctl -n hw.model 2>/dev/null || true)"
[[ "${MODEL}" == VirtualMac* ]] \
  || fail "expected an isolated VirtualMac runner; got hw.model=${MODEL:-unknown}"

for command_name in cargo codesign ffmpeg ffprobe git jq node npm openssl \
    security sqlite3 xcrun; do
  command -v "${command_name}" >/dev/null 2>&1 \
    || fail "missing hosted runner dependency: ${command_name}"
done
[[ -n "${RUNNER_TEMP:-}" && "${RUNNER_TEMP}" == /* ]] \
  || fail "RUNNER_TEMP must be an absolute path"
[[ "${CARGO_TARGET_DIR:-}" == /* ]] \
  || fail "CARGO_TARGET_DIR must be an absolute, per-job directory"
[[ ! -e "${CARGO_TARGET_DIR}" ]] \
  || fail "refusing to reuse Cargo target directory ${CARGO_TARGET_DIR}"
[[ ! -e "${LOCAL_APP}" ]] \
  || fail "refusing a runner with a pre-existing ${LOCAL_APP}"
[[ ! -e "${KEYCHAIN}" ]] \
  || fail "refusing a runner with a pre-existing signing keychain"
sudo -n -v >/dev/null 2>&1 \
  || fail "requires the hosted runner's noninteractive sudo policy"

SOURCE_SHA="$(git -C "${REPO_ROOT}" rev-parse HEAD)"
[[ "${CUA_E2E_SOURCE_SHA:-}" =~ ^[0-9a-fA-F]{40}$ ]] \
  || fail "CUA_E2E_SOURCE_SHA must be a full commit SHA"
EXPECTED_SHA="$(printf '%s' "${CUA_E2E_SOURCE_SHA}" | tr '[:upper:]' '[:lower:]')"
[[ "${SOURCE_SHA}" == "${EXPECTED_SHA}" ]] \
  || fail "checked out ${SOURCE_SHA}, expected ${CUA_E2E_SOURCE_SHA}"
[[ -z "$(git -C "${REPO_ROOT}" status --porcelain --untracked-files=normal)" ]] \
  || fail "requires a clean exact-SHA checkout"

mkdir -p "${BOOTSTRAP_DIR}"
{
  printf 'source_sha=%s\n' "${SOURCE_SHA}"
  printf 'lane=%s\n' "${LANE}"
  printf 'current_user=%s\n' "${CURRENT_USER}"
  printf 'console_user=%s\n' "${CONSOLE_USER}"
  printf 'model=%s\n' "${MODEL}"
  printf 'sip=%s\n' "${SIP_STATUS}"
  printf 'image_os=%s\n' "${ImageOS}"
  printf 'image_version=%s\n' "${ImageVersion:-unknown}"
  sw_vers
  rustc --version
  node --version
  xcode-select -p
  ffmpeg -version 2>&1 | sed -n '1p'
  ffprobe -version 2>&1 | sed -n '1p'
  jq --version
} > "${BOOTSTRAP_DIR}/environment.txt"

cat > "${BOOTSTRAP_DIR}/cleanup-targets.txt" <<EOF
Installer recursive cleanup is limited to source-derived task-owned paths:
- ${RUNNER_TEMP}/cua-driver-local/packages/releases/0.0.0-local-release-*/Skills/cua-driver
- ${RUNNER_TEMP}/cua-driver-local/packages/releases/0.0.0-local-release-*/CuaDriverLocal.app
- /Applications/CuaDriverLocal.app.install-backup.<pid>
- /Applications/CuaDriverLocal.app only while restoring a failed fresh-runner install
- mktemp signing and csreq directories under the runner's temporary directory
EOF

export CUA_MACOS_HOSTED_PROBE_DIR="${BOOTSTRAP_DIR}/gui-probe"
mark_phase "gui-probe"
bash "${SCRIPT_DIR}/probe-hosted-runner.sh"

mkdir -p "${CARGO_TARGET_DIR}"
KEYCHAIN_PASSWORD="$(openssl rand -hex 24)"
mark_phase "signing-bootstrap"
run_bounded 30 security create-keychain -p "${KEYCHAIN_PASSWORD}" "${KEYCHAIN}"
run_bounded 30 security set-keychain-settings -lut 21600 "${KEYCHAIN}"
run_bounded 30 security unlock-keychain -p "${KEYCHAIN_PASSWORD}" "${KEYCHAIN}"
while IFS= read -r keychain_entry; do
  [[ -n "${keychain_entry}" ]] && ORIGINAL_KEYCHAINS+=("${keychain_entry}")
done < <(security list-keychains -d user \
  | sed -E 's/^[[:space:]]*"//; s/"[[:space:]]*$//')
run_bounded 30 security list-keychains -d user -s \
  "${KEYCHAIN}" "${ORIGINAL_KEYCHAINS[@]}"

export CUA_DRIVER_LOCAL_HOME="${RUNNER_TEMP}/cua-driver-local"
export CUA_DRIVER_LOCAL_INSTALL_DIR="${RUNNER_TEMP}/cua-driver-bin"
export CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN="${KEYCHAIN}"
export CUA_DRIVER_SOURCE_SHA="${SOURCE_SHA}"

export OS=Darwin
export BOLD="" NORMAL="" RED="" GREEN="" BLUE="" YELLOW=""
# shellcheck disable=SC1090,SC1091
. "${DRIVER_ROOT}/scripts/_local-signing.sh"
IDENTITY="$(ensure_local_signing_identity)"
[[ "${IDENTITY}" =~ ^[0-9A-Fa-f]{40}$ ]] \
  || fail "could not create the temporary code-signing identity"
CERTIFICATE_PEM="${RUNNER_TEMP}/cua-driver-hosted-signing.pem"
security find-certificate -c "${CUA_LOCAL_SIGN_CN}" -p "${KEYCHAIN}" \
  > "${CERTIFICATE_PEM}"
run_bounded 30 sudo -n security add-trusted-cert -d -r trustRoot \
  -p codeSign -k /Library/Keychains/System.keychain "${CERTIFICATE_PEM}"
TRUSTED_IDENTITY="${IDENTITY}"
run_bounded 30 security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
  -k "${KEYCHAIN_PASSWORD}" "${KEYCHAIN}" >/dev/null
security find-identity -v -p codesigning "${KEYCHAIN}" \
  | grep -Fq "${IDENTITY}" \
  || fail "temporary identity is not valid for code signing"
export CUA_DRIVER_LOCAL_SIGNING_IDENTITY="${IDENTITY}"
printf '%s\n' "${IDENTITY}" > "${BOOTSTRAP_DIR}/signing-identity-sha1.txt"

SIGNING_PROBE="${RUNNER_TEMP}/cua-driver-signing-probe"
cp /bin/echo "${SIGNING_PROBE}"
run_bounded 30 codesign --force --sign "${IDENTITY}" \
  --keychain "${KEYCHAIN}" "${SIGNING_PROBE}"
codesign --verify --strict "${SIGNING_PROBE}"
codesign -d -r- "${SIGNING_PROBE}" \
  > "${BOOTSTRAP_DIR}/signing-probe-requirement.txt" 2>&1
grep -Fq "certificate leaf" "${BOOTSTRAP_DIR}/signing-probe-requirement.txt" \
  || fail "temporary signing probe is not certificate-backed"

mark_phase "install-local"
echo "[INSTALL] Building and installing ${SOURCE_SHA} with a temporary certificate identity"
bash "${DRIVER_ROOT}/scripts/install-local.sh" \
  --release --require-stable-signing \
  2>&1 | tee "${BOOTSTRAP_DIR}/install-local.log"

capture_command_output "${BOOTSTRAP_DIR}/codesign-requirement.txt" \
  codesign -d -r- "${LOCAL_APP}"
grep -Fq "certificate leaf" "${BOOTSTRAP_DIR}/codesign-requirement.txt" \
  || fail "installed app does not have a certificate-backed designated requirement"
codesign --verify --deep --strict "${LOCAL_APP}"

# macOS 15+ separately reminds each responsible app before direct capture,
# even when Screen Recording is granted. The hosted image pre-approves its
# runner agent, but this signed app is the responsible ScreenCaptureKit client.
echo "[CAPTURE] Suppressing the app-specific private-window-picker reminder"
mark_phase "screen-capture-approval"
mkdir -p "$(dirname "${SCREEN_CAPTURE_APPROVALS}")"
defaults write "${SCREEN_CAPTURE_APPROVALS}" "${SCREEN_CAPTURE_CLIENT}" -dict \
  kScreenCaptureApprovalLastAlerted -date "3024-01-01 00:00:00 +0000" \
  kScreenCaptureApprovalLastUsed -date "3024-01-01 00:00:00 +0000"
killall -HUP replayd >/dev/null 2>&1 || true
defaults read "${SCREEN_CAPTURE_APPROVALS}" "${SCREEN_CAPTURE_CLIENT}" \
  > "${BOOTSTRAP_DIR}/screen-capture-approval.txt"
grep -Fq "kScreenCaptureApprovalLastAlerted" \
  "${BOOTSTRAP_DIR}/screen-capture-approval.txt" \
  || fail "app-specific screen capture reminder approval was not stored"
grep -Fq "kScreenCaptureApprovalLastUsed" \
  "${BOOTSTRAP_DIR}/screen-capture-approval.txt" \
  || fail "app-specific screen capture last-used approval was not stored"

echo "[TCC] Seeding only Accessibility and Screen Capture for the installed app"
mark_phase "seed-tcc"
bash "${TCC_SEEDER}" \
  --app "${LOCAL_APP}" \
  --expected-client com.trycua.driver.local \
  2>&1 | tee "${BOOTSTRAP_DIR}/seed-tcc.log"

echo "[DAEMON] Launching the installed app in unrestricted disposable-worker mode"
mark_phase "daemon-start"
open -n -g "${LOCAL_APP}" --args \
  --socket "${DAEMON_SOCKET}" \
  serve \
  --permission-mode unrestricted \
  --dangerously-bypass-approvals
DAEMON_STARTED=1

PERMISSIONS_READY=0
for _ in {1..60}; do
  if "${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" permissions status --json \
      > "${BOOTSTRAP_DIR}/permissions.json" 2> "${BOOTSTRAP_DIR}/permissions.err" \
      && jq -e '
        .accessibility == true
        and .screen_recording == true
        and .screen_recording_capturable == null
        and .direct_capture_status == "not_checked"
        and .source.attribution == "driver-daemon"
      ' "${BOOTSTRAP_DIR}/permissions.json" >/dev/null; then
    PERMISSIONS_READY=1
    break
  fi
  sleep 1
done
[[ "${PERMISSIONS_READY}" == 1 ]] \
  || fail "installed daemon did not report the seeded app-specific permissions"
[[ -S "${DAEMON_SOCKET}" ]] || fail "installed daemon socket is missing"

"${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" call get_config '{}' \
  > "${BOOTSTRAP_DIR}/driver-config.json"
grep -Fq "${SOURCE_SHA}" "${BOOTSTRAP_DIR}/driver-config.json" \
  || fail "installed daemon did not report the requested source SHA"
"${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" list_apps '{}' \
  > "${BOOTSTRAP_DIR}/driver-list-apps.json"

watch_daemon() {
  local daemon_status
  while sleep 3; do
    daemon_status="$("${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" status 2>&1 || true)"
    if [[ "${daemon_status}" != *"permission mode: unrestricted"* ]]; then
      printf '%s daemon unavailable or not unrestricted; restarting\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      printf '%s\n' "${daemon_status}"
      "${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" stop >/dev/null 2>&1 || true
      open -n -g "${LOCAL_APP}" --args \
        --socket "${DAEMON_SOCKET}" \
        serve \
        --permission-mode unrestricted \
        --dangerously-bypass-approvals
      for _ in {1..15}; do
        sleep 1
        daemon_status="$("${INSTALLED_BIN}" --socket "${DAEMON_SOCKET}" status 2>&1 || true)"
        [[ "${daemon_status}" == *"permission mode: unrestricted"* ]] && break
      done
    fi
  done
}
watch_daemon >> "${BOOTSTRAP_DIR}/daemon-watchdog.log" 2>&1 &
WATCHDOG_PID=$!

export CUA_E2E_INSTALLED_DRIVER_BIN="${INSTALLED_BIN}"
export CUA_E2E_MACOS_DAEMON_SOCKET="${DAEMON_SOCKET}"
export CUA_E2E_FRESH_FIXTURE_STATE=1
mark_phase "matrix-${LANE}"
echo "[E2E] Running hosted macOS ${LANE} lane"
bash "${SCRIPT_DIR}/run-rust-e2e.sh"
