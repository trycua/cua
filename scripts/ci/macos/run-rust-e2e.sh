#!/usr/bin/env bash
# Run the canonical Rust desktop matrix in a logged-in macOS user session.
# macOS harness tests use the installed, TCC-authorized cua-driver daemon path.
# The install must embed CUA_DRIVER_SOURCE_SHA; the Lume wrapper owns that step.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
DRIVER_ROOT="${REPO_ROOT}/libs/cua-driver"
RUST_ROOT="${DRIVER_ROOT}/rust"
SUITE="${CUA_E2E_INTERNAL_LANE:-all}"
BUILD_FIXTURES=1

usage() {
  cat <<'EOF'
Usage: run-rust-e2e.sh [--no-build]

Run from a logged-in macOS desktop after install-local and TCC authorization.
The testkit proxies MCP calls through the installed CuaDriver daemon.
The installed daemon must embed the same CUA_DRIVER_SOURCE_SHA as this source.
Because this is a disposable behavior-test desktop, that daemon must have been
started with --permission-mode unrestricted --dangerously-bypass-approvals.
Maintainers should use libs/cua-driver/tests/runners/macos-lume/run-all.sh.
The contributor-facing command always runs the complete matrix.
EOF
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

# The Lume runner points CARGO_TARGET_DIR at an absolute, per-invocation
# namespace instead of the inherited rust/target directory. Refuse relative
# values because this script invokes Cargo from more than one working directory.
resolved_build_target_dir() {
  local configured="${CARGO_TARGET_DIR:-}"
  if [[ -z "${configured}" ]]; then
    printf '%s/target\n' "${RUST_ROOT}"
    return 0
  fi
  case "${configured}" in
    /*) printf '%s\n' "${configured}" ;;
    *)
      echo "CARGO_TARGET_DIR must be absolute for macOS E2E: ${configured}" >&2
      return 2
      ;;
  esac
}

FAILURE_COUNT=0
FAILED_LANES=()
VIDEO_FAILURES=()
PREFLIGHT_FAILED=0
REPORT_FAILED=0

note_lane_failure() {
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
  FAILED_LANES+=("$1")
}

note_video_failure() {
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
  VIDEO_FAILURES+=("$1")
}

json_bool() {
  if [[ "$1" == 1 ]]; then
    printf 'true\n'
  else
    printf 'false\n'
  fi
}

# One machine-readable record of what failed, so a caller can tell a single
# typed cell failure apart from a lane, video, preflight, or report failure.
write_failure_record() {
  local failures_file="$1"
  jq -n \
    --arg schema 'cua-driver/e2e-failures@v1' \
    --arg suite "${SUITE}" \
    --arg cell_filter "${CUA_E2E_CELL_FILTER:-}" \
    --arg harness_filter "${CUA_E2E_HARNESS_FILTER:-}" \
    --argjson failure_count "${FAILURE_COUNT}" \
    --argjson preflight_failed "$(json_bool "${PREFLIGHT_FAILED}")" \
    --argjson report_failed "$(json_bool "${REPORT_FAILED}")" \
    --argjson lanes "$(json_string_array ${FAILED_LANES[@]+"${FAILED_LANES[@]}"})" \
    --argjson video_failures "$(json_string_array ${VIDEO_FAILURES[@]+"${VIDEO_FAILURES[@]}"})" \
    '{schema: $schema, suite: $suite,
      filters: {cell: $cell_filter, harness: $harness_filter},
      failure_count: $failure_count, preflight_failed: $preflight_failed,
      report_failed: $report_failed, lanes: $lanes,
      video_failures: $video_failures}' \
    > "${failures_file}"
}

run_test() {
  local name="$1"; shift
  echo "[RUN] ${name}"
  set +e
  (cd "${RUST_ROOT}" && "$@") 2>&1 | tee "${ARTIFACT_DIR}/${name}.log"
  local exit_code=${PIPESTATUS[0]}
  set -e
  if [[ "${exit_code}" != 0 ]]; then
    note_lane_failure "${name}"
  fi
}

filter_contains_exact() {
  local filter="$1"
  local expected="$2"
  local value
  [[ -z "${filter}" ]] && return 0
  while IFS= read -r value; do
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    [[ "${value}" == "${expected}" ]] && return 0
  done < <(tr ',' '\n' <<< "${filter}")
  return 1
}

native_retry_filter_active() {
  [[ -n "${CUA_E2E_CELL_FILTER:-}" || -n "${CUA_E2E_HARNESS_FILTER:-}" ]]
}

native_swiftui_test_selected() {
  local cell="$1"
  filter_contains_exact "${CUA_E2E_HARNESS_FILTER:-}" swiftui \
    && filter_contains_exact "${CUA_E2E_CELL_FILTER:-}" "${cell}"
}

if [[ "${CUA_E2E_RUNNER_LIB_ONLY:-0}" == 1 ]]; then
  # Sourced by the focused runner tests, which exercise the helpers above
  # without a live macOS desktop.
  if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    echo "CUA_E2E_RUNNER_LIB_ONLY is valid only when this script is sourced" >&2
    exit 2
  fi
  return 0
fi

while (($#)); do
  case "$1" in
    --no-build) BUILD_FIXTURES=0 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

case "$SUITE" in
  shared|native|capture|all) ;;
  *) echo "unsupported internal lane: $SUITE" >&2; exit 2 ;;
esac

SOURCE_MARKER="${CUA_E2E_SOURCE_MARKER:-${REPO_ROOT}/.cua-e2e-source-sha}"
if git -C "${REPO_ROOT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if [[ -n "$(git -C "${REPO_ROOT}" status --porcelain --untracked-files=normal)" ]]; then
    echo "macOS canonical E2E requires a clean working tree" >&2
    exit 2
  fi
  RESOLVED_SOURCE_SHA="$(git -C "${REPO_ROOT}" rev-parse HEAD)"
elif [[ -f "${SOURCE_MARKER}" ]]; then
  RESOLVED_SOURCE_SHA="$(tr -d '[:space:]' < "${SOURCE_MARKER}")"
  export CUA_E2E_SOURCE_MARKER="${SOURCE_MARKER}"
else
  echo "macOS canonical E2E requires git metadata or ${SOURCE_MARKER}" >&2
  exit 2
fi
if [[ -z "${CUA_E2E_SOURCE_SHA:-}" ]]; then
  CUA_E2E_SOURCE_SHA="${RESOLVED_SOURCE_SHA}"
fi
if [[ ! "${CUA_E2E_SOURCE_SHA}" =~ ^[0-9a-fA-F]{40}$ ]]; then
  echo "CUA_E2E_SOURCE_SHA must be a full 40-character commit SHA" >&2
  exit 2
fi
EXPECTED_SOURCE_SHA_LOWER="$(printf '%s' "${CUA_E2E_SOURCE_SHA}" | tr '[:upper:]' '[:lower:]')"
RESOLVED_SOURCE_SHA_LOWER="$(printf '%s' "${RESOLVED_SOURCE_SHA}" | tr '[:upper:]' '[:lower:]')"
if [[ "${EXPECTED_SOURCE_SHA_LOWER}" != "${RESOLVED_SOURCE_SHA_LOWER}" ]]; then
  echo "macOS E2E source ${RESOLVED_SOURCE_SHA} does not match requested SHA ${CUA_E2E_SOURCE_SHA}" >&2
  exit 2
fi
export CUA_E2E_SOURCE_SHA
export CUA_DRIVER_SOURCE_SHA="${CUA_E2E_SOURCE_SHA}"

ARTIFACT_DIR="${REPO_ROOT}/artifacts/cua-driver/macos"
RECORDING_ROOT="${ARTIFACT_DIR}/recordings"
if [[ -e "${RECORDING_ROOT}" ]]; then
  RECORDING_ARCHIVE="$(mktemp -d "${TMPDIR:-/tmp}/cua-macos-e2e-recordings.XXXXXX")"
  mv "${RECORDING_ROOT}" "${RECORDING_ARCHIVE}/recordings"
  echo "Previous recordings preserved at ${RECORDING_ARCHIVE}/recordings"
fi
mkdir -p "${RECORDING_ROOT}"
RESULTS_FILE="${ARTIFACT_DIR}/results.jsonl"
DECLARATIONS_FILE="${ARTIFACT_DIR}/cases.jsonl"
ENVIRONMENT_FILE="${ARTIFACT_DIR}/environment.jsonl"
SUMMARY_FILE="${ARTIFACT_DIR}/summary.md"
FAILURES_FILE="${ARTIFACT_DIR}/failures.json"
mkdir -p "${ARTIFACT_DIR}"
: > "${DECLARATIONS_FILE}"
: > "${ENVIRONMENT_FILE}"
: > "${RESULTS_FILE}"
rm -f "${SUMMARY_FILE}"
# A record from a previous attempt must never describe this one.
rm -f "${FAILURES_FILE}"

export CUA_E2E_DECLARATIONS_FILE="${DECLARATIONS_FILE}"
export CUA_E2E_ENVIRONMENT_FILE="${ENVIRONMENT_FILE}"
export CUA_E2E_RESULTS_FILE="${RESULTS_FILE}"
export CUA_E2E_RECORDINGS_ROOT="${RECORDING_ROOT}"
export CUA_TEST_WORKSPACE_ROOT="${RUST_ROOT}"
BUILD_TARGET_DIR="$(resolved_build_target_dir)"
export CUA_TEST_DRIVER_BIN="${BUILD_TARGET_DIR}/release/cua-driver"
export CUA_TEST_APPS_ROOT="${RUST_ROOT}/test-apps"
export CUA_TEST_REQUIRE_FIXTURES=1
export CUA_TEST_DRIVER_STDERR=1
export CUA_E2E_FORBID_SKIPS=1
unset CUA_E2E_EXPECTED_MIN_CELLS
if [[ ("${SUITE}" == shared || "${SUITE}" == all) \
  && -z "${CUA_E2E_CELL_FILTER:-}" \
  && -z "${CUA_E2E_HARNESS_FILTER:-}" ]]; then
  export CUA_E2E_EXPECTED_MIN_CELLS=120
fi

command -v ffmpeg >/dev/null || { echo "ffmpeg is required for E2E trajectory videos" >&2; exit 1; }
command -v ffprobe >/dev/null || { echo "ffprobe is required for E2E trajectory validation" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is required for E2E ownership validation" >&2; exit 1; }

if [[ "${BUILD_FIXTURES}" == 1 ]]; then
  cargo build --release -p cua-driver --manifest-path "${RUST_ROOT}/Cargo.toml"
  bash "${DRIVER_ROOT}/tests/fixtures/build/macos.sh"
fi

if [[ ! -x "${CUA_TEST_DRIVER_BIN}" ]]; then
  echo "Required driver binary was not built: ${CUA_TEST_DRIVER_BIN}" >&2
  exit 1
fi
MACOS_DAEMON_SOCKET="${CUA_E2E_MACOS_DAEMON_SOCKET:-${HOME}/Library/Caches/cua-driver/cua-driver.sock}"
MACOS_DAEMON_BIN="${CUA_E2E_INSTALLED_DRIVER_BIN:-${CUA_TEST_DRIVER_BIN}}"
if ! output_contains "permission mode: unrestricted" \
    "${MACOS_DAEMON_BIN}" status --socket "${MACOS_DAEMON_SOCKET}"; then
  printf '%s\n' "${CAPTURED_OUTPUT}" >&2
  echo "macOS canonical E2E requires an explicitly unrestricted disposable worker daemon" >&2
  echo "Use tests/runners/macos-lume/run-all.sh, which restores standard mode afterward" >&2
  exit 1
fi

required_fixtures=()
required_fixtures+=("${CUA_TEST_APPS_ROOT}/harness-electron/CuaTestHarness.Electron.app")
if [[ "${SUITE}" == shared || "${SUITE}" == all ]]; then
  required_fixtures+=(
    "${CUA_TEST_APPS_ROOT}/harness-tauri/CuaTestHarness.Tauri.app"
    "${CUA_TEST_APPS_ROOT}/harness-wkwebview/CuaTestHarness.WKWebView.app"
  )
fi
if [[ "${SUITE}" == native || "${SUITE}" == all ]]; then
  required_fixtures+=(
    "${CUA_TEST_APPS_ROOT}/harness-appkit/CuaTestHarness.AppKit.app"
    "${CUA_TEST_APPS_ROOT}/harness-swiftui/CuaTestHarness.SwiftUI.app"
  )
fi
for fixture in "${required_fixtures[@]}"; do
  [[ -d "${fixture}" ]] || { echo "Required fixture missing: ${fixture}" >&2; exit 1; }
done

run_report() {
  (cd "${RUST_ROOT}" && cargo run -p cua-driver-testkit --bin cua-e2e-report -- \
    --declarations "${DECLARATIONS_FILE}" \
    --environment "${ENVIRONMENT_FILE}" \
    --results "${RESULTS_FILE}" \
    --artifact-root "${ARTIFACT_DIR}" \
    --require-video \
    --output "${SUMMARY_FILE}")
}

echo "[PREFLIGHT] macOS daemon identity, fixture, AX, capture, and video"
set +e
(cd "${RUST_ROOT}" && cargo test -p cua-driver --test e2e_environment_preflight_test -- \
  --ignored --exact canonical_e2e_environment_is_ready --nocapture --test-threads=1) \
  2>&1 | tee "${ARTIFACT_DIR}/environment-preflight.log"
PREFLIGHT_EXIT=${PIPESTATUS[0]}
set -e
if [[ "${PREFLIGHT_EXIT}" != 0 ]]; then
  PREFLIGHT_FAILED=1
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
  set +e
  run_report
  set -e
  write_failure_record "${FAILURES_FILE}"
  echo "macOS E2E environment preflight failed" >&2
  exit 1
fi

if [[ "${SUITE}" == shared || "${SUITE}" == all ]]; then
  run_test protected-permission-prompt-socket cargo test -p cua-driver \
    --test permission_prompt_authorization_test -- --test-threads=1
  run_test protected-host-self-launch cargo test -p platform-macos \
    launch_app::tests -- --test-threads=1
  run_test sdk-runtime-contract cargo test -p cua-driver-sdk --lib -- --test-threads=1
  run_test sdk-runtime-configuration cargo test -p cua-driver-sdk \
    --test runtime_configuration -- --test-threads=1
  run_test private-worker-lifecycle cargo test -p cua-driver \
    --test private_worker_test -- --test-threads=1
  run_test shared-app-matrix cargo test -p cua-driver --test cross_platform_behavior_test -- \
    --ignored --exact shared_web_action_matrix_is_state_verified \
    --nocapture --test-threads=1
  run_test embedded-browser-routes cargo test -p cua-driver --test cross_platform_behavior_test -- \
    --ignored --exact embedded_browser_routes_are_exact_or_refused \
    --nocapture --test-threads=1
fi
if [[ "${SUITE}" == native || "${SUITE}" == all ]]; then
  NATIVE_FILTER_MATCHES=0
  if ! native_retry_filter_active; then
    run_test agent-cursor-showcase cargo test -p cua-driver \
      --test agent_cursor_showcase_test -- \
      --ignored --nocapture --test-threads=1
    for appkit_test in \
    harness_appkit_smoke \
    harness_appkit_query_projects_structured_elements \
    harness_appkit_stale_element_token_fails_closed \
    harness_appkit_invoke_menu_live_path \
    harness_appkit_text_input \
    harness_appkit_element_foreground_press_key_commits_edit \
    harness_appkit_modified_click_preserves_selection \
    harness_appkit_type_text_background \
    harness_appkit_scroll_foreground \
    harness_appkit_scroll_background \
    harness_appkit_counter \
    harness_appkit_counter_px_background \
    harness_appkit_right_click_px_foreground \
    harness_appkit_right_click_px_background \
    harness_appkit_double_click_px_foreground \
    harness_appkit_double_click_px_background \
    harness_appkit_slider_drag_px_foreground \
      harness_appkit_slider_drag_px_background; do
      run_test "appkit-${appkit_test}" cargo test -p cua-driver --test harness_appkit_test -- \
        --ignored --exact "${appkit_test}" --nocapture --test-threads=1
    done
  fi

  while IFS='|' read -r swiftui_cell swiftui_test; do
    if native_swiftui_test_selected "${swiftui_cell}"; then
      NATIVE_FILTER_MATCHES=$((NATIVE_FILTER_MATCHES + 1))
      run_test "swiftui-${swiftui_test}" cargo test -p cua-driver --test harness_swiftui_test -- \
        --ignored --exact "${swiftui_test}" --nocapture --test-threads=1
    fi
  done <<'EOF'
macos-swiftui-ax-tree-ax-not-applicable|harness_swiftui_smoke
macos-swiftui-left-click-ax-background|harness_swiftui_counter_background
macos-swiftui-set-value-ax-background|harness_swiftui_set_value_background
macos-swiftui-popover-open-ax-foreground|harness_swiftui_popover_foreground
macos-swiftui-verify-state-ax-not-applicable|harness_swiftui_verify_state
EOF

  if native_retry_filter_active; then
    if ((NATIVE_FILTER_MATCHES == 0)); then
      echo "No native test owns the requested harness/cell filter" >&2
      note_lane_failure native-filter-selection
    fi
  else
    run_test installed-app-launch cargo test -p cua-driver --test installed_app_launch_macos_test -- \
      --ignored --nocapture --test-threads=1
    run_test installed-app-textedit cargo test -p cua-driver --test installed_app_textedit_macos_test -- \
      --ignored --exact background_type_on_native_cocoa_is_ax_verified \
      --nocapture --test-threads=1
    run_test installed-app-textedit-rebind cargo test -p cua-driver --test installed_app_textedit_macos_test -- \
      --ignored --exact background_open_panel_returns_a_typed_rebind \
      --nocapture --test-threads=1
  fi
fi
if [[ "${SUITE}" == capture || "${SUITE}" == all ]]; then
  run_test capture-contract cargo test -p cua-driver --test capture_contract_test -- \
    --ignored --nocapture --test-threads=1
  run_test desktop-scope cargo test -p cua-driver --test desktop_scope_macos_test -- \
    --ignored --nocapture --test-threads=1
fi

video_count=0
while IFS= read -r -d '' video; do
  video_count=$((video_count + 1))
  if ! ffprobe -v error -show_entries format=duration \
      -of default=noprint_wrappers=1:nokey=1 "${video}" >/dev/null; then
    echo "[VIDEO FAIL] Unplayable trajectory: ${video}" >&2
    note_video_failure "unplayable:${video#"${ARTIFACT_DIR}/"}"
  fi
done < <(find "${RECORDING_ROOT}" -type f -name recording.mp4 -print0)

OWNED_VIDEOS="$(mktemp)"
jq -r 'select(.evidence.video != null) | .evidence.video' "${RESULTS_FILE}" > "${OWNED_VIDEOS}"
while IFS= read -r -d '' video; do
  relative="${video#"${ARTIFACT_DIR}/"}"
  if [[ "${relative}" == recordings/environment-preflight-*/recording.mp4 ]]; then
    continue
  fi
  if ! grep -Fxq -- "${relative}" "${OWNED_VIDEOS}"; then
    echo "[VIDEO FAIL] Orphan trajectory has no typed result row: ${relative}" >&2
    note_video_failure "orphan:${relative}"
  fi
done < <(find "${RECORDING_ROOT}" -type f -name recording.mp4 -print0)
rm -f "${OWNED_VIDEOS}"

while IFS= read -r -d '' error_file; do
  echo "[VIDEO FAIL] ${error_file}" >&2
  cat "${error_file}" >&2
  note_video_failure "recording-error:${error_file#"${ARTIFACT_DIR}/"}"
done < <(find "${RECORDING_ROOT}" -type f -name recording-error.txt -print0)

if [[ "${video_count}" == 0 ]]; then
  echo "[VIDEO FAIL] No E2E trajectory videos were produced" >&2
  note_video_failure "no-trajectories"
fi

set +e
run_report
REPORT_EXIT=$?
set -e
if [[ "${REPORT_EXIT}" != 0 ]]; then
  echo "macOS E2E result validation failed" >&2
  REPORT_FAILED=1
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
fi

write_failure_record "${FAILURES_FILE}"

if [[ "${FAILURE_COUNT}" != 0 ]]; then
  echo "macOS Rust E2E suite had ${FAILURE_COUNT} failure signal(s)" >&2
  exit 1
fi
echo "macOS Rust E2E suite completed: ${SUITE}"
