#!/usr/bin/env bash
# Run the canonical Rust desktop matrix on a Linux user session.
# Scenario definitions and assertions live in the Rust integration test.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
DRIVER_ROOT="${REPO_ROOT}/libs/cua-driver"
RUST_ROOT="${DRIVER_ROOT}/rust"
BUILD_FIXTURES=1
SUITE="shared"

usage() {
  cat <<'EOF'
Usage: run-rust-e2e.sh [--no-build] [--suite shared|modality|all]

The caller must provide a real or virtual Linux desktop session. For a
headless session, wrap this command in xvfb-run and dbus-run-session.
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
  shared|modality|all) ;;
  *) echo "unsupported suite: $SUITE" >&2; exit 2 ;;
esac

mkdir -p "${REPO_ROOT}/artifacts/cua-driver/linux"
RECORDING_ROOT="${REPO_ROOT}/artifacts/cua-driver/linux/recordings"
rm -rf "${RECORDING_ROOT}"
mkdir -p "${RECORDING_ROOT}"
RESULTS_FILE="${REPO_ROOT}/artifacts/cua-driver/linux/results.jsonl"
SUMMARY_FILE="${REPO_ROOT}/artifacts/cua-driver/linux/summary.md"
: > "${RESULTS_FILE}"
cat > "${SUMMARY_FILE}" <<'EOF'
# CUA Rust Linux E2E matrix

| Platform | Host/lane | Scenario | Status | Duration | Details |
| --- | --- | --- | --- | --- | --- |
EOF
export CUA_E2E_RESULTS_FILE="${RESULTS_FILE}"
export CUA_E2E_SUMMARY_FILE="${SUMMARY_FILE}"
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
  bash "${DRIVER_ROOT}/tests/fixtures/build/linux.sh"
fi

if [[ ! -x "${CUA_TEST_DRIVER_BIN}" ]]; then
  echo "driver binary not found: ${CUA_TEST_DRIVER_BIN}" >&2
  exit 1
fi
if [[ ! -x "${CUA_TEST_APPS_ROOT}/harness-electron/CuaTestHarness.Electron" ]]; then
  echo "Electron fixture was not built: ${CUA_TEST_APPS_ROOT}/harness-electron" >&2
  exit 1
fi

FAILURE_COUNT=0

run_test() {
  local name="$1"
  shift
  echo "[RUN] ${name}"
  set +e
  (cd "${RUST_ROOT}" && "$@") 2>&1 | tee "${REPO_ROOT}/artifacts/cua-driver/linux/${name}.log"
  local exit_code=${PIPESTATUS[0]}
  set -e
  local status="PASS"
  if [[ "${exit_code}" != 0 ]]; then
    status="FAIL"
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
  fi
  local details="-"
  if [[ "${exit_code}" != 0 ]]; then
    details="exit code ${exit_code}"
  fi
  printf '{"schema":"cua-e2e-result/v1","platform":"linux","host":"lane","scenario":"%s","status":"%s","message":"%s"}\n' \
    "${name}" "${status}" "${details}" >> "${RESULTS_FILE}"
  printf '| Linux | lane | %s | %s | n/a | %s |\n' "${name}" "${status}" "${details}" >> "${SUMMARY_FILE}"
}

if [[ "${SUITE}" == shared || "${SUITE}" == all ]]; then
  run_test shared-behavior-matrix \
    cargo test -p cua-driver --test cross_platform_behavior_test -- \
      --ignored --nocapture --test-threads=1
fi

if [[ "${SUITE}" == modality || "${SUITE}" == all ]]; then
  run_test modality-capture \
    cargo test -p cua-driver --test modality_capture_mode_test -- \
      --ignored --nocapture --test-threads=1
  run_test modality-desktop-scope \
    cargo test -p cua-driver --test modality_desktop_scope_linux_test -- \
      --ignored --nocapture --test-threads=1
fi

video_count=0
while IFS= read -r -d '' video; do
  video_count=$((video_count + 1))
  if ! ffprobe -v error -show_entries format=duration \
      -of default=noprint_wrappers=1:nokey=1 "${video}" >/dev/null; then
    echo "[VIDEO FAIL] Unplayable trajectory: ${video}" >&2
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
  else
    echo "[VIDEO PASS] ${video}"
  fi
done < <(find "${RECORDING_ROOT}" -type f -name recording.mp4 -print0)

while IFS= read -r -d '' recording_error; do
  echo "[VIDEO FAIL] ${recording_error}" >&2
  cat "${recording_error}" >&2
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
done < <(find "${RECORDING_ROOT}" -type f -name recording-error.txt -print0)

if [[ "${video_count}" == 0 ]]; then
  echo "[VIDEO FAIL] No E2E trajectory videos were produced" >&2
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
fi

if [[ "${FAILURE_COUNT}" != 0 ]]; then
  echo "Linux Rust e2e suite had ${FAILURE_COUNT} failing lane(s)" >&2
  exit 1
fi

echo "Linux Rust e2e suite completed: ${SUITE}"
