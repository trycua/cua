#!/usr/bin/env bash
# Run the canonical Rust desktop matrix in a logged-in macOS user session.
# macOS harness tests use the installed, TCC-authorized cua-driver daemon path.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
DRIVER_ROOT="${REPO_ROOT}/libs/cua-driver"
RUST_ROOT="${DRIVER_ROOT}/rust"
SUITE="all"
BUILD_FIXTURES=1

usage() {
  cat <<'EOF'
Usage: run-rust-e2e.sh [--no-build] [--suite shared|native|modality|all]

Run from a logged-in macOS desktop after install-local and TCC authorization.
The testkit proxies MCP calls through the installed CuaDriver daemon.
EOF
}

while (($#)); do
  case "$1" in
    --no-build) BUILD_FIXTURES=0 ;;
    --suite) SUITE="${2:?missing suite}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

case "$SUITE" in
  shared|native|modality|all) ;;
  *) echo "unsupported suite: $SUITE" >&2; exit 2 ;;
esac

ARTIFACT_DIR="${REPO_ROOT}/artifacts/cua-driver/macos"
RECORDING_ROOT="${ARTIFACT_DIR}/recordings"
rm -rf "${RECORDING_ROOT}"
mkdir -p "${RECORDING_ROOT}"
RESULTS_FILE="${ARTIFACT_DIR}/results.jsonl"
DECLARATIONS_FILE="${ARTIFACT_DIR}/cases.jsonl"
ENVIRONMENT_FILE="${ARTIFACT_DIR}/environment.jsonl"
SUMMARY_FILE="${ARTIFACT_DIR}/summary.md"
mkdir -p "${ARTIFACT_DIR}"
: > "${DECLARATIONS_FILE}"
: > "${ENVIRONMENT_FILE}"
: > "${RESULTS_FILE}"
rm -f "${SUMMARY_FILE}"

export CUA_E2E_DECLARATIONS_FILE="${DECLARATIONS_FILE}"
export CUA_E2E_ENVIRONMENT_FILE="${ENVIRONMENT_FILE}"
export CUA_E2E_RESULTS_FILE="${RESULTS_FILE}"
export CUA_E2E_RECORDINGS_ROOT="${RECORDING_ROOT}"
export CUA_TEST_WORKSPACE_ROOT="${RUST_ROOT}"
export CUA_TEST_DRIVER_BIN="${RUST_ROOT}/target/release/cua-driver"
export CUA_TEST_APPS_ROOT="${RUST_ROOT}/test-apps"
export CUA_TEST_REQUIRE_FIXTURES=1
export CUA_TEST_DRIVER_STDERR=1

command -v ffmpeg >/dev/null || { echo "ffmpeg is required for E2E trajectory videos" >&2; exit 1; }
command -v ffprobe >/dev/null || { echo "ffprobe is required for E2E trajectory validation" >&2; exit 1; }

if [[ "${BUILD_FIXTURES}" == 1 ]]; then
  cargo build --release -p cua-driver --manifest-path "${RUST_ROOT}/Cargo.toml"
  bash "${DRIVER_ROOT}/tests/fixtures/build/macos.sh"
fi

if [[ ! -x "${CUA_TEST_DRIVER_BIN}" ]]; then
  echo "Required driver binary was not built: ${CUA_TEST_DRIVER_BIN}" >&2
  exit 1
fi

required_fixtures=()
required_fixtures+=("${CUA_TEST_APPS_ROOT}/harness-electron/CuaTestHarness.Electron.app")
if [[ "${SUITE}" == shared || "${SUITE}" == all ]]; then
  required_fixtures+=(
    "${CUA_TEST_APPS_ROOT}/harness-tauri/CuaTestHarness.Tauri.app"
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

FAILURE_COUNT=0

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
  set +e
  run_report
  set -e
  echo "macOS E2E environment preflight failed" >&2
  exit 1
fi

run_test() {
  local name="$1"; shift
  echo "[RUN] ${name}"
  set +e
  (cd "${RUST_ROOT}" && "$@") 2>&1 | tee "${ARTIFACT_DIR}/${name}.log"
  local exit_code=${PIPESTATUS[0]}
  set -e
  if [[ "${exit_code}" != 0 ]]; then
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
  fi
}

if [[ "${SUITE}" == shared || "${SUITE}" == all ]]; then
  run_test shared-app-matrix cargo test -p cua-driver --test cross_platform_behavior_test -- \
    --ignored --exact shared_web_action_matrix_is_state_verified \
    --nocapture --test-threads=1
fi
if [[ "${SUITE}" == native || "${SUITE}" == all ]]; then
  for appkit_test in \
    harness_appkit_smoke \
    harness_appkit_text_input \
    harness_appkit_type_text_keystroke \
    harness_appkit_counter; do
    run_test "appkit-${appkit_test}" cargo test -p cua-driver --test harness_appkit_test -- \
      --ignored --exact "${appkit_test}" --nocapture --test-threads=1
  done
  run_test swiftui-native-harness cargo test -p cua-driver --test harness_swiftui_test -- \
    --ignored --nocapture --test-threads=1
fi
if [[ "${SUITE}" == modality || "${SUITE}" == all ]]; then
  run_test capture-contract cargo test -p cua-driver --test capture_contract_test -- \
    --ignored --nocapture --test-threads=1
  run_test modality-desktop-scope cargo test -p cua-driver --test modality_desktop_scope_macos_test -- \
    --ignored --nocapture --test-threads=1
fi

video_count=0
while IFS= read -r -d '' video; do
  video_count=$((video_count + 1))
  if ! ffprobe -v error -show_entries format=duration \
      -of default=noprint_wrappers=1:nokey=1 "${video}" >/dev/null; then
    echo "[VIDEO FAIL] Unplayable trajectory: ${video}" >&2
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
  fi
done < <(find "${RECORDING_ROOT}" -type f -name recording.mp4 -print0)

while IFS= read -r -d '' error_file; do
  echo "[VIDEO FAIL] ${error_file}" >&2
  cat "${error_file}" >&2
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
done < <(find "${RECORDING_ROOT}" -type f -name recording-error.txt -print0)

if [[ "${video_count}" == 0 ]]; then
  echo "[VIDEO FAIL] No E2E trajectory videos were produced" >&2
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
fi

set +e
run_report
REPORT_EXIT=$?
set -e
if [[ "${REPORT_EXIT}" != 0 ]]; then
  echo "macOS E2E result validation failed" >&2
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
fi

if [[ "${FAILURE_COUNT}" != 0 ]]; then
  echo "macOS Rust E2E suite had ${FAILURE_COUNT} failing lane(s)" >&2
  exit 1
fi
echo "macOS Rust E2E suite completed: ${SUITE}"
