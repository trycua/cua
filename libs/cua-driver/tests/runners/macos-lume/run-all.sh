#!/usr/bin/env bash
# Install and run the canonical macOS GUI matrix inside a disposable Lume clone.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# A copied runner may target a clean, exact source tree during release certification.
REPO_ROOT="${CUA_E2E_REPO_ROOT:-$(cd "${SCRIPT_DIR}/../../../../.." && pwd)}"
DRIVER_ROOT="${REPO_ROOT}/libs/cua-driver"
RUST_ROOT="${DRIVER_ROOT}/rust"
ARTIFACT_DIR="${REPO_ROOT}/artifacts/cua-driver/macos"
ARTIFACT_HISTORY_ROOT="${REPO_ROOT}/artifacts/cua-driver/macos-history"
SOURCE_MARKER="${CUA_E2E_SOURCE_MARKER:-${REPO_ROOT}/.cua-e2e-source-sha}"
SIGNING_KEYCHAIN="${CUA_E2E_SIGNING_KEYCHAIN:-${HOME}/Library/Keychains/cua-driver-signing.keychain-db}"
SIGNING_CN="CuaDriver Local Signing (cua-driver-rs)"
LOCAL_APP="/Applications/CuaDriverLocal.app"
# install-local.sh intentionally uses a separate namespace from release installs.
INSTALLED_BIN="${HOME}/.local/bin/cua-driver-local"
LOCAL_PLIST="${HOME}/Library/LaunchAgents/com.trycua.cua-driver-local.plist"
CUA_E2E_MACOS_DAEMON_SOCKET="${CUA_E2E_MACOS_DAEMON_SOCKET:-${HOME}/Library/Caches/cua-driver-local/cua-driver-local.sock}"
# A run-owned Cargo namespace keeps a certification build off the seed image's
# and any other commit's target state without deleting a shared cache.
CARGO_TARGET_ROOT="${CUA_E2E_CARGO_TARGET_ROOT:-${HOME}/Library/Caches/cua-driver-e2e/cargo-target}"
# Only the shared web-action lanes honor cell and harness filters, so a targeted
# retry always reruns that internal lane.
RETRY_INTERNAL_LANE=shared
RETRY_ATTEMPTS_LIMIT=3
# How long to wait for a daemon mode transition, in one-second polls.
DAEMON_MODE_WAIT_ATTEMPTS="${CUA_E2E_DAEMON_WAIT_ATTEMPTS:-10}"
# Lanes whose failure is attributable to one typed matrix cell that an exact
# single-cell rerun can reproduce. The embedded-browser lane is excluded on
# purpose: filtering to one of its cells leaves the shared web-action lane with
# no selected cells, which that lane reports as a failure of its own.
RETRYABLE_LANES=(shared-app-matrix)

usage() {
  cat <<'EOF'
Usage: run-all.sh [--no-build] [--standalone-browser]
                  [--retry-cell <cell-id> [--retry-harness <harness>]
                   [--retry-attempts <1-3>] [--retry-only]]

Run the canonical cua-driver macOS GUI E2E matrix in a disposable clone of
the maintainer Lume golden image. Start this command from Terminal in the VM's
logged-in desktop session, not over SSH.

--standalone-browser also runs the optional installed Chrome/Edge browser-tool
matrix after the canonical repo-local harness matrix.

--retry-cell authorizes exactly one bounded retry selection. After a failing
full matrix the runner retries that single cell only when it was the matrix's
only failure, preserves the first-attempt evidence under
artifacts/cua-driver/macos/attempts/, and records the initial failure plus every
retry attempt in retry-record.json and summary.md. Any other failure, and a cell
that keeps failing, still fails the run.

--retry-harness names the harness that owns the retried cell and must match the
failing row. --retry-attempts bounds the retries (1-3, default 1). --retry-only
skips the full matrix and runs just the selection, starting and verifying the
unrestricted worker daemon first.
EOF
}

RUN_STANDALONE_BROWSER=0
NO_BUILD=0
RETRY_CELL=""
RETRY_HARNESS=""
RETRY_ATTEMPTS=""
RETRY_ONLY=0

parse_arguments() {
  RUN_STANDALONE_BROWSER=0
  NO_BUILD=0
  RETRY_CELL=""
  RETRY_HARNESS=""
  RETRY_ATTEMPTS=""
  RETRY_ONLY=0
  while (($#)); do
    case "$1" in
      --no-build) NO_BUILD=1 ;;
      --standalone-browser) RUN_STANDALONE_BROWSER=1 ;;
      --retry-only) RETRY_ONLY=1 ;;
      --retry-cell=*) RETRY_CELL="${1#*=}" ;;
      --retry-harness=*) RETRY_HARNESS="${1#*=}" ;;
      --retry-attempts=*) RETRY_ATTEMPTS="${1#*=}" ;;
      --retry-cell|--retry-harness|--retry-attempts)
        if (($# < 2)) || [[ -z "$2" || "$2" == -* ]]; then
          echo "$1 requires a value" >&2
          return 2
        fi
        case "$1" in
          --retry-cell) RETRY_CELL="$2" ;;
          --retry-harness) RETRY_HARNESS="$2" ;;
          --retry-attempts) RETRY_ATTEMPTS="$2" ;;
        esac
        shift
        ;;
      -h|--help) usage; return 3 ;;
      *) echo "unknown argument: $1" >&2; return 2 ;;
    esac
    shift
  done
  validate_arguments
}

validate_arguments() {
  if [[ -z "${RETRY_CELL}" ]]; then
    if [[ -n "${RETRY_HARNESS}" ]]; then
      echo "--retry-harness requires --retry-cell" >&2
      return 2
    fi
    if [[ -n "${RETRY_ATTEMPTS}" ]]; then
      echo "--retry-attempts requires --retry-cell" >&2
      return 2
    fi
    if [[ "${RETRY_ONLY}" == 1 ]]; then
      echo "--retry-only requires --retry-cell" >&2
      return 2
    fi
    return 0
  fi

  # One exact cell keeps the retry auditable; a list cannot be matched against a
  # single failing row.
  if [[ ! "${RETRY_CELL}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
    echo "--retry-cell must be one artifact-safe cell id, got: ${RETRY_CELL}" >&2
    return 2
  fi
  if [[ -n "${RETRY_HARNESS}" && ! "${RETRY_HARNESS}" =~ ^[a-z0-9][a-z0-9-]*$ ]]; then
    echo "--retry-harness must be one harness name, got: ${RETRY_HARNESS}" >&2
    return 2
  fi
  if [[ -z "${RETRY_ATTEMPTS}" ]]; then
    RETRY_ATTEMPTS=1
  fi
  if [[ ! "${RETRY_ATTEMPTS}" =~ ^[1-9][0-9]*$ ]] \
      || ((RETRY_ATTEMPTS > RETRY_ATTEMPTS_LIMIT)); then
    echo "--retry-attempts must be between 1 and ${RETRY_ATTEMPTS_LIMIT}" >&2
    return 2
  fi
  if [[ "${RETRY_ONLY}" == 1 && "${RUN_STANDALONE_BROWSER}" == 1 ]]; then
    echo "--retry-only cannot be combined with --standalone-browser" >&2
    return 2
  fi
}

# Capture a command's full output before matching it. Piping a producer into
# `grep -q` closes its stdout mid-write, the driver CLI can panic on that broken
# pipe, and `pipefail` then reports the panic as the pipeline's status even when
# the pattern matched.
CAPTURED_OUTPUT=""
capture_command_output() {
  local status=0
  CAPTURED_OUTPUT=""
  CAPTURED_OUTPUT="$("$@" 2>&1)" || status=$?
  return "${status}"
}

output_contains() {
  local needle="$1"
  shift
  local status=0
  capture_command_output "$@" || status=$?
  if ((status != 0)); then
    return 1
  fi
  [[ "${CAPTURED_OUTPUT}" == *"${needle}"* ]]
}

json_string_array() {
  local item
  local result=''
  for item in "$@"; do
    result="${result:+${result},}$(jq -n --arg value "${item}" '$value')"
  done
  printf '[%s]\n' "${result}"
}

resolve_cargo_target_dir() {
  local source_sha="$1"
  local run_id="$2"
  if [[ "${CARGO_TARGET_ROOT}" != /* ]]; then
    echo "CUA_E2E_CARGO_TARGET_ROOT must be an absolute path" >&2
    return 2
  fi
  local normalized_sha
  normalized_sha="$(printf '%s' "${source_sha}" | tr '[:upper:]' '[:lower:]')"
  if [[ ! "${normalized_sha}" =~ ^[0-9a-f]{40}$ ]]; then
    echo "refusing to derive a build namespace from ${source_sha}" >&2
    return 2
  fi
  if [[ ! "${run_id}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
    echo "refusing unsafe run id: ${run_id}" >&2
    return 2
  fi
  printf '%s/%s/%s\n' "${CARGO_TARGET_ROOT}" "${normalized_sha}" "${run_id}"
}

preserve_previous_artifacts() {
  local run_id="$1"
  if [[ ! -d "${ARTIFACT_DIR}" ]] \
      || [[ -z "$(find "${ARTIFACT_DIR}" -mindepth 1 -print -quit)" ]]; then
    mkdir -p "${ARTIFACT_DIR}"
    return 0
  fi
  local destination="${ARTIFACT_HISTORY_ROOT}/${run_id}"
  if [[ -e "${destination}" ]]; then
    echo "refusing to overwrite an existing artifact history: ${destination}" >&2
    return 2
  fi
  mkdir -p "${ARTIFACT_HISTORY_ROOT}"
  mv "${ARTIFACT_DIR}" "${destination}"
  mkdir -p "${ARTIFACT_DIR}"
  echo "[EVIDENCE] Preserved the previous certification run at ${destination}"
}

RESTORE_STANDARD_DAEMON=0
UNRESTRICTED_WATCHDOG_PID=""

daemon_reports_permission_mode() {
  local mode="$1"
  output_contains "permission mode: ${mode}" \
    "${INSTALLED_BIN}" status --socket "${CUA_E2E_MACOS_DAEMON_SOCKET}"
}

wait_for_permission_mode() {
  local mode="$1"
  local attempts="${2:-10}"
  local attempt
  for ((attempt = 1; attempt <= attempts; attempt++)); do
    if daemon_reports_permission_mode "${mode}"; then
      return 0
    fi
    sleep 1
  done
  return 1
}

stop_worker_daemon() {
  "${INSTALLED_BIN}" stop --socket "${CUA_E2E_MACOS_DAEMON_SOCKET}" >/dev/null 2>&1 || true
}

start_unrestricted_daemon() {
  open -n -g "${LOCAL_APP}" --args \
    serve \
    --permission-mode unrestricted \
    --dangerously-bypass-approvals \
    >/dev/null 2>&1
}

keep_unrestricted_daemon_alive() {
  while true; do
    watchdog_check_once
    sleep 3
  done
}

watchdog_check_once() {
  if daemon_reports_permission_mode unrestricted; then
    return 0
  fi
  printf '%s daemon status probe failed; confirming before restart\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '%s\n' "${CAPTURED_OUTPUT}"
  if ! wait_for_permission_mode unrestricted 3; then
    printf '%s daemon was unavailable or used the wrong mode; restarting\n' \
      "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '%s\n' "${CAPTURED_OUTPUT}"
    stop_worker_daemon
    start_unrestricted_daemon
    if ! wait_for_permission_mode unrestricted "${DAEMON_MODE_WAIT_ATTEMPTS}"; then
      printf '%s daemon restart did not reach unrestricted mode\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      printf '%s\n' "${CAPTURED_OUTPUT}"
    fi
  fi
  return 0
}

start_unrestricted_watchdog() {
  if [[ -n "${UNRESTRICTED_WATCHDOG_PID}" ]]; then
    return 0
  fi
  keep_unrestricted_daemon_alive \
    >> "${ARTIFACT_DIR}/unrestricted-daemon-watchdog.log" 2>&1 &
  UNRESTRICTED_WATCHDOG_PID=$!
}

stop_unrestricted_watchdog() {
  if [[ -z "${UNRESTRICTED_WATCHDOG_PID}" ]]; then
    return 0
  fi
  kill "${UNRESTRICTED_WATCHDOG_PID}" >/dev/null 2>&1 || true
  wait "${UNRESTRICTED_WATCHDOG_PID}" 2>/dev/null || true
  UNRESTRICTED_WATCHDOG_PID=""
}

# Every matrix attempt, including a targeted retry, claims the worker daemon
# through this one owner, so no attempt can run against the restored standard
# daemon or against a mode this runner did not verify.
ensure_unrestricted_daemon() {
  if [[ "${RESTORE_STANDARD_DAEMON}" == 1 ]] && daemon_reports_permission_mode unrestricted; then
    start_unrestricted_watchdog
    return 0
  fi
  echo "[AUTHORIZATION] Starting the disposable worker daemon in unrestricted mode"
  stop_unrestricted_watchdog
  launchctl unload "${LOCAL_PLIST}" 2>/dev/null || true
  stop_worker_daemon
  RESTORE_STANDARD_DAEMON=1
  start_unrestricted_daemon
  if ! wait_for_permission_mode unrestricted "${DAEMON_MODE_WAIT_ATTEMPTS}"; then
    printf '%s\n' "${CAPTURED_OUTPUT}" >&2
    echo "The macOS behavior matrix could not start its unrestricted worker daemon" >&2
    return 1
  fi
  start_unrestricted_watchdog
}

restore_standard_daemon() {
  local command_status=$?
  trap - EXIT
  stop_unrestricted_watchdog
  if [[ "${RESTORE_STANDARD_DAEMON}" != 1 ]]; then
    exit "${command_status}"
  fi
  set +e
  echo "[AUTHORIZATION] Restoring the worker's standard autostart daemon"
  stop_worker_daemon
  if ! launchctl print "gui/$(id -u)" >/dev/null 2>&1; then
    if [[ ! -f "${LOCAL_PLIST}" ]]; then
      echo "The standard autostart LaunchAgent is missing: ${LOCAL_PLIST}" >&2
      exit 1
    fi
    {
      printf 'status=deferred-until-next-gui-login\n'
      printf 'exit_status=%s\n' "${command_status}"
    } > "${ARTIFACT_DIR}/standard-daemon-restore-deferred.txt"
    echo "[AUTHORIZATION] The GUI login session ended; the standard LaunchAgent will load at the next login"
    RESTORE_STANDARD_DAEMON=0
    exit "${command_status}"
  fi
  launchctl load "${LOCAL_PLIST}"
  if ! wait_for_permission_mode standard "${DAEMON_MODE_WAIT_ATTEMPTS}"; then
    printf '%s\n' "${CAPTURED_OUTPUT}" >&2
    echo "The disposable worker could not restore its standard autostart daemon" >&2
    if [[ "${command_status}" == 0 ]]; then
      command_status=1
    fi
  fi
  RESTORE_STANDARD_DAEMON=0
  set -e
  exit "${command_status}"
}

install_daemon_restore_traps() {
  trap restore_standard_daemon EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
}

ATTEMPT_EVIDENCE_PATH=""
# Move the evidence an attempt left at the artifact root into its own directory
# so the next attempt cannot overwrite it. Recordings move; the typed rows,
# summary, and logs are copied, so preserving evidence never deletes anything.
archive_attempt_evidence() {
  local label="$1"
  local destination="${ARTIFACT_DIR}/attempts/${label}"
  local entry
  mkdir -p "${destination}"
  if [[ -d "${ARTIFACT_DIR}/recordings" ]]; then
    mv "${ARTIFACT_DIR}/recordings" "${destination}/recordings"
  fi
  shopt -s nullglob
  for entry in "${ARTIFACT_DIR}"/*.jsonl "${ARTIFACT_DIR}"/*.log \
      "${ARTIFACT_DIR}/summary.md" "${ARTIFACT_DIR}/failures.json"; do
    cp -p "${entry}" "${destination}/"
  done
  shopt -u nullglob
  ATTEMPT_EVIDENCE_PATH="attempts/${label}"
  echo "[EVIDENCE] Preserved attempt evidence at ${ATTEMPT_EVIDENCE_PATH}"
}

failing_cells_from_results() {
  local results_file="$1"
  [[ -f "${results_file}" ]] || return 1
  jq -r 'select(.test_status != "pass") | .cell_id' "${results_file}"
}

RETRY_INITIAL_CELLS=()
RETRY_INITIAL_LANES_JSON="[]"

# A single-cell retry may only repair a single-cell failure. Anything else in the
# initial attempt would be hidden by a green retry, so it stays fatal.
retry_selection_is_eligible() {
  local results_file="$1"
  local failures_file="$2"
  local cell
  RETRY_INITIAL_CELLS=()
  RETRY_INITIAL_LANES_JSON="[]"

  local failing_cells=""
  if ! failing_cells="$(failing_cells_from_results "${results_file}")"; then
    echo "[RETRY] Refusing the retry: no typed result rows at ${results_file}" >&2
    return 1
  fi
  while IFS= read -r cell; do
    if [[ -n "${cell}" ]]; then
      RETRY_INITIAL_CELLS+=("${cell}")
    fi
  done <<< "${failing_cells}"
  if ((${#RETRY_INITIAL_CELLS[@]} != 1)) \
      || [[ "${RETRY_INITIAL_CELLS[0]}" != "${RETRY_CELL}" ]]; then
    echo "[RETRY] Refusing the retry: initial failing cells (${RETRY_INITIAL_CELLS[*]:-none}) are not exactly ${RETRY_CELL}" >&2
    return 1
  fi
  if [[ -n "${RETRY_HARNESS}" ]]; then
    local failing_harness
    failing_harness="$(jq -r --arg cell "${RETRY_CELL}" \
      'select(.test_status != "pass" and .cell_id == $cell) | .harness' "${results_file}")"
    if [[ "${failing_harness}" != "${RETRY_HARNESS}" ]]; then
      echo "[RETRY] Refusing the retry: ${RETRY_CELL} belongs to harness ${failing_harness:-unknown}, not ${RETRY_HARNESS}" >&2
      return 1
    fi
  fi
  if [[ ! -f "${failures_file}" ]]; then
    echo "[RETRY] Refusing the retry: the matrix wrote no machine-readable failure record" >&2
    return 1
  fi
  local reason
  reason="$(jq -r --argjson retryable "$(json_string_array "${RETRYABLE_LANES[@]}")" '
    if .preflight_failed then "the environment preflight failed"
    elif .report_failed then "the typed result report validation failed"
    elif (.video_failures | length) > 0 then
      "trajectory video validation failed: \(.video_failures | join(", "))"
    elif .failure_count != 1 then
      "\(.failure_count) failure signals were recorded"
    elif (.lanes | length) != 1 then
      "\(.lanes | length) lanes failed: \(.lanes | join(", "))"
    elif ((.lanes[0]) as $lane | ($retryable | index($lane)) == null) then
      "lane \(.lanes[0]) is not retryable by an exact single-cell rerun"
    else "" end' "${failures_file}")"
  if [[ -n "${reason}" ]]; then
    echo "[RETRY] Refusing the retry: ${reason}" >&2
    return 1
  fi
  RETRY_INITIAL_LANES_JSON="$(jq -c '.lanes' "${failures_file}")"
}

run_full_matrix() {
  local status=0
  local -a matrix_command=("${REPO_ROOT}/scripts/ci/macos/run-rust-e2e.sh")
  if [[ "${NO_BUILD}" == 1 ]]; then
    matrix_command+=(--no-build)
  fi
  (
    unset CUA_E2E_CELL_FILTER CUA_E2E_HARNESS_FILTER CUA_E2E_INTERNAL_LANE
    "${matrix_command[@]}"
  ) || status=$?
  return "${status}"
}

RETRY_BUILD_DONE=0
run_retry_matrix() {
  local status=0
  local -a matrix_command=("${REPO_ROOT}/scripts/ci/macos/run-rust-e2e.sh")
  # The first retry attempt of a --retry-only invocation still needs fixtures.
  if [[ "${NO_BUILD}" == 1 || "${RETRY_BUILD_DONE}" == 1 ]]; then
    matrix_command+=(--no-build)
  fi
  CUA_E2E_INTERNAL_LANE="${RETRY_INTERNAL_LANE}" \
    CUA_E2E_CELL_FILTER="${RETRY_CELL}" \
    CUA_E2E_HARNESS_FILTER="${RETRY_HARNESS}" \
    "${matrix_command[@]}" || status=$?
  if ! retry_result_is_exact; then
    echo "[RETRY] The filtered run did not produce exactly the selected cell ${RETRY_CELL}" >&2
    status=1
  fi
  RETRY_BUILD_DONE=1
  return "${status}"
}

retry_result_is_exact() {
  local results_file="${ARTIFACT_DIR}/results.jsonl"
  [[ -f "${results_file}" ]] || return 1
  jq -s -e --arg cell "${RETRY_CELL}" \
    'length == 1 and .[0].cell_id == $cell' "${results_file}" >/dev/null
}

RETRY_EXIT_CODES=()
retry_attempts_json() {
  local total=${#RETRY_EXIT_CODES[@]}
  local index evidence
  local -a records=()
  if ((total == 0)); then
    printf '[]\n'
    return 0
  fi
  for ((index = 0; index < total; index++)); do
    # Only the last attempt still owns the artifact root; earlier ones were
    # archived before the next attempt started.
    if ((index == total - 1)); then
      evidence="."
    else
      evidence="attempts/retry-$((index + 1))"
    fi
    records+=("$(jq -n \
      --argjson attempt "$((index + 1))" \
      --argjson exit_code "${RETRY_EXIT_CODES[index]}" \
      --arg evidence "${evidence}" \
      '{attempt: $attempt, exit_code: $exit_code,
        status: (if $exit_code == 0 then "pass" else "fail" end),
        evidence: $evidence}')")
  done
  printf '%s\n' "${records[@]}" | jq -s '.'
}

retry_mode_name() {
  if [[ "${RETRY_ONLY}" == 1 ]]; then
    printf 'retry-only\n'
  else
    printf 'after-full-matrix\n'
  fi
}

write_retry_record() {
  local final_status="$1"
  local initial_json="$2"
  local attempts_json
  attempts_json="$(retry_attempts_json)"
  jq -n \
    --arg schema 'cua-driver/e2e-retry-record@v1' \
    --arg source_sha "${CUA_E2E_SOURCE_SHA:-}" \
    --arg mode "$(retry_mode_name)" \
    --arg cell "${RETRY_CELL}" \
    --arg harness "${RETRY_HARNESS}" \
    --arg lane "${RETRY_INTERNAL_LANE}" \
    --argjson allowed_attempts "${RETRY_ATTEMPTS}" \
    --argjson initial_attempt "${initial_json}" \
    --argjson retry_attempts "${attempts_json}" \
    --arg final_status "${final_status}" \
    '{schema: $schema, source_sha: $source_sha, mode: $mode,
      selection: {cell_id: $cell, harness: $harness, internal_lane: $lane,
                  allowed_attempts: $allowed_attempts},
      initial_attempt: $initial_attempt, retry_attempts: $retry_attempts,
      final_status: $final_status}' \
    > "${ARTIFACT_DIR}/retry-record.json"

  # The regenerated summary only describes the last attempt, so state the
  # initial failure in the human-facing report as well.
  {
    printf '\n## Retry provenance\n\n'
    printf -- '- Retry selection: cell `%s`' "${RETRY_CELL}"
    if [[ -n "${RETRY_HARNESS}" ]]; then
      printf ', harness `%s`' "${RETRY_HARNESS}"
    fi
    printf ', internal lane `%s`, up to %s attempt(s)\n' \
      "${RETRY_INTERNAL_LANE}" "${RETRY_ATTEMPTS}"
    if [[ "${RETRY_ONLY}" == 1 ]]; then
      printf -- '- Initial full matrix: not run in this invocation (--retry-only)\n'
    else
      printf -- '- Initial full matrix: FAILED on `%s` (evidence: `attempts/attempt-1-full-matrix`)\n' \
        "${RETRY_CELL}"
    fi
    printf -- '- Retry attempts: %s\n' \
      "$(jq -r '[.[] | "#\(.attempt) \(.status) (evidence: \(.evidence))"] | join("; ")' \
        <<< "${attempts_json}")"
    printf -- '- Final status: %s\n' "${final_status}"
  } >> "${ARTIFACT_DIR}/summary.md"
  echo "[RETRY] Recorded ${final_status} in ${ARTIFACT_DIR}/retry-record.json"
}

# Run the authorized selection until it passes or the bounded attempts run out.
run_retry_sequence() {
  local attempt status
  RETRY_EXIT_CODES=()
  for ((attempt = 1; attempt <= RETRY_ATTEMPTS; attempt++)); do
    if ((attempt > 1)); then
      archive_attempt_evidence "retry-$((attempt - 1))"
    fi
    echo "[RETRY] Attempt ${attempt} of ${RETRY_ATTEMPTS} for cell ${RETRY_CELL}"
    if ! ensure_unrestricted_daemon; then
      RETRY_EXIT_CODES+=(1)
      return 1
    fi
    status=0
    run_retry_matrix || status=$?
    RETRY_EXIT_CODES+=("${status}")
    if ((status == 0)); then
      return 0
    fi
    echo "[RETRY] Attempt ${attempt} failed with exit ${status}" >&2
  done
  return 1
}

if [[ "${CUA_E2E_RUNNER_LIB_ONLY:-0}" == 1 ]]; then
  # Sourced by the focused runner tests, which exercise the helpers above
  # without a macOS GUI session.
  if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    echo "CUA_E2E_RUNNER_LIB_ONLY is valid only when this script is sourced" >&2
    exit 2
  fi
  return 0
fi

PARSE_STATUS=0
parse_arguments "$@" || PARSE_STATUS=$?
if ((PARSE_STATUS == 3)); then
  exit 0
elif ((PARSE_STATUS != 0)); then
  usage >&2
  exit 2
fi

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
if [[ "${SIP_STATUS}" != *"System Integrity Protection status: disabled."* ]]; then
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
if ! output_contains "\"${SIGNING_CN}\"" \
    security find-identity -v -p codesigning "${SIGNING_KEYCHAIN}"; then
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
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
export CUA_E2E_RUN_ID="${RUN_ID}"
export CUA_E2E_FRESH_FIXTURE_STATE=1
preserve_previous_artifacts "${RUN_ID}"
printf '%s\n' "${SIP_STATUS}" > "${ARTIFACT_DIR}/sip-status.txt"
printf '%s\n' "${SOURCE_SHA}" > "${ARTIFACT_DIR}/requested-source-sha.txt"
{
  sw_vers
  printf 'console_user:\t%s\n' "${CONSOLE_USER}"
  printf 'rustc:\t%s\n' "$(rustc --version)"
  printf 'node:\t%s\n' "$(node --version)"
  printf 'xcode_select:\t%s\n' "$(xcode-select -p)"
} > "${ARTIFACT_DIR}/golden-environment.txt"

# Keep this invocation's Cargo output out of the seed image and every previous
# run, including a prior run of the same source SHA. Nothing here deletes or
# mutates an existing build directory.
CARGO_TARGET_DIR="$(resolve_cargo_target_dir "${SOURCE_SHA}" "${RUN_ID}")"
export CARGO_TARGET_DIR
if [[ -e "${CARGO_TARGET_DIR}" ]]; then
  echo "refusing to reuse an existing Cargo target directory: ${CARGO_TARGET_DIR}" >&2
  exit 2
fi
mkdir -p "${CARGO_TARGET_DIR}"
printf '%s\n' "${CARGO_TARGET_DIR}" > "${ARTIFACT_DIR}/cargo-target-dir.txt"
echo "[BUILD NAMESPACE] Building in ${CARGO_TARGET_DIR}"

echo "[AUTOMATION] Verifying Terminal can read frontmost state through System Events"
if ! osascript -e \
    'tell application "System Events" to get name of first application process whose frontmost is true' \
    > "${ARTIFACT_DIR}/terminal-system-events.txt"; then
  echo "Terminal cannot control System Events; rebuild the seed and grant the Automation prompt" >&2
  exit 2
fi

echo "[INSTALL] Building and installing ${SOURCE_SHA} with the golden signing identity"
bash "${DRIVER_ROOT}/scripts/install-local.sh" \
  --release --autostart --require-stable-signing \
  2>&1 | tee "${ARTIFACT_DIR}/install-local.log"

codesign -d -r- "${LOCAL_APP}" \
  > "${ARTIFACT_DIR}/codesign-requirement.txt" 2>&1
if ! grep -Fq "certificate leaf" "${ARTIFACT_DIR}/codesign-requirement.txt"; then
  echo "CuaDriverLocal.app is not signed with the golden image's stable certificate identity" >&2
  exit 1
fi

export CUA_E2E_INSTALLED_DRIVER_BIN="${INSTALLED_BIN}"
export CUA_E2E_MACOS_DAEMON_SOCKET
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

install_daemon_restore_traps
ensure_unrestricted_daemon

echo "[APP DISCOVERY] Verifying the installed CuaDriver can enumerate apps"
if ! "${INSTALLED_BIN}" list_apps '{}' > "${ARTIFACT_DIR}/driver-list-apps.json"; then
  echo "CuaDriver cannot enumerate applications" >&2
  exit 2
fi

RESULTS_FILE="${ARTIFACT_DIR}/results.jsonl"
FAILURES_FILE="${ARTIFACT_DIR}/failures.json"

if [[ "${RETRY_ONLY}" == 1 ]]; then
  echo "[E2E] Running only the authorized retry selection for ${RETRY_CELL}"
  RETRY_STATUS=0
  run_retry_sequence || RETRY_STATUS=$?
  if [[ "${RETRY_STATUS}" != 0 ]]; then
    write_retry_record 'fail-retry-only' 'null'
    echo "The authorized retry selection ${RETRY_CELL} failed every attempt" >&2
    exit 1
  fi
  write_retry_record 'pass-retry-only' 'null'
  echo "macOS retry-only run passed the authorized selection ${RETRY_CELL}"
  exit 0
fi

echo "[E2E] Running the canonical macOS matrix"
MATRIX_STATUS=0
run_full_matrix || MATRIX_STATUS=$?
# The full matrix left this commit's binaries and fixtures in place, so a retry
# does not need to rebuild them.
RETRY_BUILD_DONE=1

if [[ "${MATRIX_STATUS}" != 0 ]]; then
  if [[ -z "${RETRY_CELL}" ]]; then
    exit "${MATRIX_STATUS}"
  fi
  if ! retry_selection_is_eligible "${RESULTS_FILE}" "${FAILURES_FILE}"; then
    echo "The macOS matrix failure is not the authorized single-cell retry selection" >&2
    exit "${MATRIX_STATUS}"
  fi
  INITIAL_ATTEMPT_JSON="$(jq -n \
    --argjson failing_cells "$(json_string_array "${RETRY_INITIAL_CELLS[@]}")" \
    --argjson lanes "${RETRY_INITIAL_LANES_JSON}" \
    --argjson exit_code "${MATRIX_STATUS}" \
    '{status: "fail", exit_code: $exit_code, failing_cells: $failing_cells,
      lanes: $lanes, evidence: "attempts/attempt-1-full-matrix"}')"
  archive_attempt_evidence attempt-1-full-matrix
  RETRY_STATUS=0
  run_retry_sequence || RETRY_STATUS=$?
  if [[ "${RETRY_STATUS}" != 0 ]]; then
    write_retry_record 'fail-persistent' "${INITIAL_ATTEMPT_JSON}"
    echo "The macOS matrix cell ${RETRY_CELL} failed its initial attempt and every retry" >&2
    exit 1
  fi
  write_retry_record 'pass-after-retry' "${INITIAL_ATTEMPT_JSON}"
  echo "macOS matrix passed after retrying only ${RETRY_CELL}; the initial failure is recorded"
elif [[ -n "${RETRY_CELL}" ]]; then
  echo "[RETRY] The full matrix passed; no retry of ${RETRY_CELL} was needed"
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
  ensure_unrestricted_daemon
  set +e
  CUA_E2E_ARTIFACT_DIR="${BROWSER_ARTIFACT_DIR}" \
    "${REPO_ROOT}/scripts/ci/run-rust-standalone-browser-e2e.sh"
  BROWSER_STATUS=$?
  set -e

  if [[ "${BROWSER_STATUS}" != 0 ]]; then
    exit "${BROWSER_STATUS}"
  fi
fi
