#!/usr/bin/env bash
# Run the cross-platform external Chromium browser matrix in the caller's desktop session.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
RUST_ROOT="${REPO_ROOT}/libs/cua-driver/rust"
ARTIFACT_DIR="${CUA_E2E_ARTIFACT_DIR:-${REPO_ROOT}/artifacts/cua-driver/standalone-browser}"

if [[ -d "${ARTIFACT_DIR}" ]] \
    && [[ -n "$(find "${ARTIFACT_DIR}" -mindepth 1 -print -quit)" ]]; then
  echo "Standalone-browser artifact directory is not empty: ${ARTIFACT_DIR}" >&2
  echo "Use a fresh CUA_E2E_ARTIFACT_DIR so rows and videos cannot be mixed across runs." >&2
  exit 2
fi

mkdir -p "${ARTIFACT_DIR}/recordings"
export CUA_E2E_DECLARATIONS_FILE="${ARTIFACT_DIR}/cases.jsonl"
export CUA_E2E_ENVIRONMENT_FILE="${ARTIFACT_DIR}/environment.jsonl"
export CUA_E2E_RESULTS_FILE="${ARTIFACT_DIR}/results.jsonl"
export CUA_E2E_RECORDINGS_ROOT="${ARTIFACT_DIR}/recordings"
export CUA_TEST_WORKSPACE_ROOT="${RUST_ROOT}"
export CUA_TEST_DRIVER_BIN="${CUA_TEST_DRIVER_BIN:-${RUST_ROOT}/target/release/cua-driver}"
export CUA_TEST_REQUIRE_EXTERNAL_BROWSERS=1
export CUA_E2E_FORBID_SKIPS=1
export CUA_TEST_DRIVER_STDERR=1
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

HOST_OS="$(uname -s)"
if [[ "${HOST_OS}" == Linux ]] \
    && [[ "${XDG_SESSION_TYPE:-}" == wayland ]] \
    && [[ -n "${WAYLAND_DISPLAY:-}" ]] \
    && [[ -z "${DISPLAY:-}" ]]; then
  # This lane promises native Wayland behavior. Opt into the driver's native
  # backend explicitly so a missing caller variable cannot silently exercise
  # the X11 fallback and time out during native-window correlation.
  export CUA_DRIVER_RS_ENABLE_WAYLAND=1
  export ELECTRON_OZONE_PLATFORM_HINT=wayland
fi

case "${HOST_OS}" in
  Darwin)
    SENTINEL_FIXTURE="${RUST_ROOT}/test-apps/harness-electron/CuaTestHarness.Electron.app"
    ;;
  Linux)
    SENTINEL_FIXTURE="${RUST_ROOT}/test-apps/harness-electron/CuaTestHarness.Electron"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    SENTINEL_FIXTURE="${RUST_ROOT}/test-apps/harness-electron/CuaTestHarness.Electron.exe"
    ;;
  *)
    echo "Unsupported standalone-browser host: ${HOST_OS}" >&2
    exit 2
    ;;
esac

if [[ ! -e "${SENTINEL_FIXTURE}" ]]; then
  echo "[FIXTURE] Staging the Electron foreground sentinel"
  if [[ "${HOST_OS}" == MINGW* || "${HOST_OS}" == MSYS* || "${HOST_OS}" == CYGWIN* ]]; then
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \
      "${REPO_ROOT}/libs/cua-driver/tests/fixtures/apps/cross-platform/electron/build.ps1"
  else
    "${REPO_ROOT}/libs/cua-driver/tests/fixtures/apps/cross-platform/electron/build.sh"
  fi
fi
if [[ ! -e "${SENTINEL_FIXTURE}" ]]; then
  echo "Electron foreground sentinel was not staged: ${SENTINEL_FIXTURE}" >&2
  exit 1
fi

: > "${CUA_E2E_DECLARATIONS_FILE}"
: > "${CUA_E2E_ENVIRONMENT_FILE}"
: > "${CUA_E2E_RESULTS_FILE}"

SOURCE_MARKER="${REPO_ROOT}/.cua-e2e-source-sha"
if [[ -f "${SOURCE_MARKER}" ]]; then
  export CUA_E2E_SOURCE_MARKER="${CUA_E2E_SOURCE_MARKER:-${SOURCE_MARKER}}"
  if [[ -z "${CUA_E2E_SOURCE_SHA:-}" ]]; then
    export CUA_E2E_SOURCE_SHA="$(tr -d '[:space:]' < "${SOURCE_MARKER}")"
  fi
fi

if [[ "${CUA_E2E_SKIP_BUILD:-0}" != 1 ]]; then
  cargo build --release -p cua-driver --manifest-path "${RUST_ROOT}/Cargo.toml"
fi
if [[ ! -x "${CUA_TEST_DRIVER_BIN}" ]]; then
  echo "Driver binary not found: ${CUA_TEST_DRIVER_BIN}" >&2
  exit 1
fi

tests=(
  standalone_browser_background_type
  standalone_browser_dialogs
  standalone_browser_download
  standalone_browser_existing_profile
  standalone_browser_existing_profile_setup
  standalone_browser_frames
  standalone_browser_multi_tab
  standalone_browser_pointer_actions
  standalone_browser_prepare_isolated
  standalone_browser_roundtrip
  standalone_browser_semantic_state
  standalone_browser_stale_ref
  standalone_browser_trusted_click
  standalone_browser_upload
  standalone_browser_window_collision
)
failure_count=0
for test_name in "${tests[@]}"; do
  echo "[RUN] ${test_name}"
  set +e
  (cd "${RUST_ROOT}" && cargo test --release -p cua-driver \
    --test standalone_browser_behavior_test "${test_name}" -- \
    --ignored --exact --nocapture --test-threads=1) \
    2>&1 | tee "${ARTIFACT_DIR}/${test_name}.log"
  test_status=${PIPESTATUS[0]}
  set -e
  if [[ "${test_status}" != 0 ]]; then
    failure_count=$((failure_count + 1))
  fi
done

set +e
(cd "${RUST_ROOT}" && cargo run --release -p cua-driver-testkit \
  --bin cua-e2e-report -- \
  --declarations "${CUA_E2E_DECLARATIONS_FILE}" \
  --environment "${CUA_E2E_ENVIRONMENT_FILE}" \
  --results "${CUA_E2E_RESULTS_FILE}" \
  --artifact-root "${ARTIFACT_DIR}" \
  --require-video \
  --output "${ARTIFACT_DIR}/summary.md")
report_status=$?
set -e
if [[ "${report_status}" != 0 ]]; then
  failure_count=$((failure_count + 1))
fi

if [[ "${failure_count}" != 0 ]]; then
  echo "Standalone-browser E2E had ${failure_count} failing step(s)" >&2
  exit 1
fi

echo "Standalone-browser E2E completed"
