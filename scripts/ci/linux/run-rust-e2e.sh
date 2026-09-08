#!/usr/bin/env bash
# Run the canonical Rust desktop matrix on a Linux user session.
# Scenario definitions and assertions live in the Rust integration test.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
DRIVER_ROOT="${REPO_ROOT}/libs/cua-driver"
RUST_ROOT="${DRIVER_ROOT}/rust"
BUILD_FIXTURES=1
SUITE="${CUA_E2E_INTERNAL_LANE:-all}"

usage() {
  cat <<'EOF'
Usage: run-rust-e2e.sh [--no-build]

The caller must provide a real or virtual Linux desktop session. For a
headless session, wrap this command in xvfb-run and dbus-run-session.
The contributor-facing command always runs the complete matrix.
EOF
}

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

ARTIFACT_DIR="${REPO_ROOT}/artifacts/cua-driver/linux"
mkdir -p "${ARTIFACT_DIR}"
RECORDING_ROOT="${ARTIFACT_DIR}/recordings"
rm -rf "${RECORDING_ROOT}"
mkdir -p "${RECORDING_ROOT}"
DECLARATIONS_FILE="${ARTIFACT_DIR}/cases.jsonl"
ENVIRONMENT_FILE="${ARTIFACT_DIR}/environment.jsonl"
RESULTS_FILE="${ARTIFACT_DIR}/results.jsonl"
SUMMARY_FILE="${ARTIFACT_DIR}/summary.md"
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
export CUA_E2E_FORBID_SKIPS=1
# The canonical behavior matrix runs inside a disposable CI desktop and must
# exercise protected GUI operations without interactive approvals. This
# testkit-only switch authorizes its spawned behavior daemons without leaking
# product authorization variables into SDK/runtime tests in the same runner.
# Focused tests can still select standard/bounded mode explicitly.
export CUA_E2E_UNRESTRICTED_GUI=1
# This runner contract requires a real or virtual desktop. Make GUI-dependent
# lifecycle proofs fail instead of silently returning without evidence.
export CUA_REQUIRE_GUI=1
unset CUA_E2E_EXPECTED_MIN_CELLS
if [[ ("${SUITE}" == shared || "${SUITE}" == all) \
  && -z "${CUA_E2E_CELL_FILTER:-}" \
  && -z "${CUA_E2E_HARNESS_FILTER:-}" ]]; then
  export CUA_E2E_EXPECTED_MIN_CELLS=80
fi
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
SOURCE_MARKER="${REPO_ROOT}/.cua-e2e-source-sha"
if [[ -f "${SOURCE_MARKER}" ]]; then
  export CUA_E2E_SOURCE_MARKER="${CUA_E2E_SOURCE_MARKER:-${SOURCE_MARKER}}"
  if [[ -z "${CUA_E2E_SOURCE_SHA:-}" ]]; then
    export CUA_E2E_SOURCE_SHA="$(tr -d '[:space:]' < "${SOURCE_MARKER}")"
  fi
fi
if [[ -n "${CUA_E2E_SOURCE_SHA:-}" ]]; then
  export CUA_DRIVER_SOURCE_SHA="${CUA_E2E_SOURCE_SHA}"
fi
if [[ -n "${WAYLAND_DISPLAY:-}" && -z "${DISPLAY:-}" ]]; then
  export GDK_BACKEND="${GDK_BACKEND:-wayland}"
  export CUA_E2E_COMPOSITOR="${CUA_E2E_COMPOSITOR:-wayland-unknown}"
  export CUA_E2E_INPUT_BACKENDS="${CUA_E2E_INPUT_BACKENDS:-atspi}"
else
  export CUA_E2E_COMPOSITOR="${CUA_E2E_COMPOSITOR:-openbox-x11}"
  export CUA_E2E_INPUT_BACKENDS="${CUA_E2E_INPUT_BACKENDS:-atspi,xsend-event,xtest}"
fi
CARGO_DRIVER_FEATURE_ARGS=()
if [[ ",${CUA_E2E_INPUT_BACKENDS}," == *,libei-portal,* ]]; then
  # GNOME and KDE retain DISPLAY for XWayland while the product route remains
  # native Wayland. Their representative lanes require the release-shipped
  # RemoteDesktop/libei adapter, so every cua-driver build/test in this runner
  # must compile the same portal-input feature instead of falling back to wtype.
  CARGO_DRIVER_FEATURE_ARGS=(--features portal-input)
fi
if [[ "${SUITE}" == shared || "${SUITE}" == all ]]; then
  export CUA_ATSPI_DEBUG=1
fi

if [[ -n "${WAYLAND_DISPLAY:-}" && -z "${DISPLAY:-}" ]]; then
  command -v wf-recorder >/dev/null || { echo "wf-recorder is required for native Wayland E2E videos" >&2; exit 1; }
  command -v grim >/dev/null || { echo "grim is required for native Wayland capture fallback" >&2; exit 1; }
  command -v wtype >/dev/null || { echo "wtype is required for native Wayland keyboard input" >&2; exit 1; }
else
  command -v ffmpeg >/dev/null || { echo "ffmpeg is required for X11 E2E trajectory videos" >&2; exit 1; }
fi
command -v ffprobe >/dev/null || { echo "ffprobe is required for E2E trajectory validation" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is required for E2E ownership validation" >&2; exit 1; }

if [[ "${BUILD_FIXTURES}" == 1 ]]; then
  cargo build --release -p cua-driver \
    "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
    --manifest-path "${RUST_ROOT}/Cargo.toml"
  case "${SUITE}" in
    shared) FIXTURE_TARGETS="${CUA_E2E_HARNESS_FILTER:-electron,tauri}" ;;
    native)
      if [[ -n "${WAYLAND_DISPLAY:-}" && -z "${DISPLAY:-}" ]]; then
        FIXTURE_TARGETS="electron,gtk3"
      else
        FIXTURE_TARGETS="electron,gtk3,gtk4"
      fi
      ;;
    capture) FIXTURE_TARGETS="electron,gtk3" ;;
    *)
      FIXTURE_TARGETS="${CUA_E2E_HARNESS_FILTER:-electron,tauri},gtk3"
      if [[ -z "${WAYLAND_DISPLAY:-}" || -n "${DISPLAY:-}" ]]; then
        FIXTURE_TARGETS+=",gtk4"
      fi
      ;;
  esac
  bash "${DRIVER_ROOT}/tests/fixtures/build/linux.sh" --only "${FIXTURE_TARGETS}"
fi

if [[ ! -x "${CUA_TEST_DRIVER_BIN}" ]]; then
  echo "driver binary not found: ${CUA_TEST_DRIVER_BIN}" >&2
  exit 1
fi
required_fixtures=()
required_fixtures+=("${CUA_TEST_APPS_ROOT}/harness-electron/CuaTestHarness.Electron")
if [[ ("${SUITE}" == shared || "${SUITE}" == all) \
  && ",${CUA_E2E_HARNESS_FILTER:-electron,tauri}," == *,tauri,* ]]; then
  required_fixtures+=(
    "${CUA_TEST_APPS_ROOT}/harness-tauri/CuaTestHarness.Tauri"
  )
fi
if [[ "${SUITE}" == native || "${SUITE}" == all ]]; then
  required_fixtures+=(
    "${CUA_TEST_APPS_ROOT}/harness-gtk3/CuaTestHarness.Gtk3"
  )
  if [[ -z "${WAYLAND_DISPLAY:-}" || -n "${DISPLAY:-}" ]]; then
    required_fixtures+=("${CUA_TEST_APPS_ROOT}/harness-gtk4/CuaTestHarness.Gtk4")
  fi
fi
for fixture in "${required_fixtures[@]}"; do
  if [[ ! -x "${fixture}" ]]; then
    echo "Required fixture was not built: ${fixture}" >&2
    exit 1
  fi
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

echo "[PREFLIGHT] Linux desktop, fixture, AX, capture, and video"
set +e
(cd "${RUST_ROOT}" && cargo test -p cua-driver \
  "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
  --test e2e_environment_preflight_test -- \
  --ignored --exact canonical_e2e_environment_is_ready --nocapture --test-threads=1) \
  2>&1 | tee "${ARTIFACT_DIR}/environment-preflight.log"
PREFLIGHT_EXIT=${PIPESTATUS[0]}
set -e
if [[ "${PREFLIGHT_EXIT}" != 0 ]]; then
  set +e
  run_report
  set -e
  echo "Linux E2E environment preflight failed" >&2
  exit 1
fi

run_test() {
  local name="$1"
  shift
  echo "[RUN] ${name}"
  set +e
  (cd "${RUST_ROOT}" && "$@") 2>&1 | tee "${ARTIFACT_DIR}/${name}.log"
  local exit_code=${PIPESTATUS[0]}
  set -e
  if [[ "${exit_code}" != 0 ]]; then
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
  fi
}

run_computer_history_gate() {
  local history_home="${ARTIFACT_DIR}/history-product-home"
  local history_bin_dir="${ARTIFACT_DIR}/history-product-bin"
  local history_socket="${ARTIFACT_DIR}/history-daemon.sock"
  export CUA_DRIVER_LOCAL_HOME="${history_home}"
  export CUA_DRIVER_LOCAL_INSTALL_DIR="${history_bin_dir}"
  export XDG_STATE_HOME="${ARTIFACT_DIR}/history-state"

  echo "[HISTORY] Installing the exact candidate into an isolated local namespace"
  bash "${DRIVER_ROOT}/scripts/install-local.sh" --release \
    2>&1 | tee "${ARTIFACT_DIR}/history-install-local.log"
  export CUA_E2E_INSTALLED_DRIVER_BIN="${history_home}/packages/current/cua-driver-local"
  export CUA_E2E_HISTORY_DAEMON_SOCKET="${history_socket}"
  if [[ ! -x "${CUA_E2E_INSTALLED_DRIVER_BIN}" ]]; then
    echo "installed history driver is missing: ${CUA_E2E_INSTALLED_DRIVER_BIN}" >&2
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
    return
  fi

  if ! "${CUA_E2E_INSTALLED_DRIVER_BIN}" history purge-offline --yes \
      >"${ARTIFACT_DIR}/history-purge-preflight.log" 2>&1; then
    echo "installed history driver could not establish an empty encrypted store" >&2
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
    return
  fi

  "${CUA_E2E_INSTALLED_DRIVER_BIN}" serve \
    --socket "${history_socket}" \
    --permission-mode unrestricted \
    --dangerously-bypass-approvals \
    >"${ARTIFACT_DIR}/history-daemon.log" 2>&1 &
  local daemon_pid=$!
  local ready=0
  for _ in $(seq 1 150); do
    if [[ -S "${history_socket}" ]]; then
      ready=1
      break
    fi
    if ! kill -0 "${daemon_pid}" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if [[ "${ready}" != 1 ]]; then
    echo "installed history daemon did not become ready" >&2
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
    return
  fi

  run_test computer-history-encrypted-lifecycle \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test computer_history_cross_platform_test -- \
      --ignored --exact encrypted_history_survives_restart_and_cryptographically_purges \
      --nocapture --test-threads=1

  set +e
  "${CUA_E2E_INSTALLED_DRIVER_BIN}" stop --socket "${history_socket}" \
    >>"${ARTIFACT_DIR}/history-daemon.log" 2>&1
  wait "${daemon_pid}" 2>/dev/null
  set -e
}

if [[ "${SUITE}" == shared || "${SUITE}" == all ]]; then
  run_test protected-permission-prompt-socket \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test permission_prompt_authorization_test -- --test-threads=1
  run_test sdk-runtime-contract \
    cargo test -p cua-driver-sdk --lib -- --test-threads=1
  run_test sdk-runtime-configuration \
    cargo test -p cua-driver-sdk --test runtime_configuration -- --test-threads=1
  run_test private-worker-lifecycle \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test private_worker_test -- --test-threads=1
  run_test shared-behavior-matrix \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test cross_platform_behavior_test -- \
      --ignored --exact shared_web_action_matrix_is_state_verified \
      --nocapture --test-threads=1
  run_test embedded-browser-routes \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test cross_platform_behavior_test -- \
      --ignored --exact embedded_browser_routes_are_exact_or_refused \
      --nocapture --test-threads=1
fi

if [[ "${SUITE}" == native || "${SUITE}" == all ]]; then
  run_test wayland-overlay-idle-no-overlay \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test wayland_overlay_idle_test -- \
      --ignored --exact no_overlay_flag_never_starts_wayland_overlay_thread \
      --nocapture --test-threads=1
  if [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
    run_test wayland-overlay-idle-recovery \
      cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
        --test wayland_overlay_idle_test -- \
        --ignored --exact wayland_overlay_quiesces_and_recovers_after_capture_and_cursor_activity \
        --nocapture --test-threads=1
  fi
  run_test agent-cursor-showcase \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test agent_cursor_showcase_test -- \
      --ignored --nocapture --test-threads=1
  run_test gtk3-native-harness \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test harness_gtk3_test -- \
      --ignored --nocapture --test-threads=1
  if [[ -z "${WAYLAND_DISPLAY:-}" || -n "${DISPLAY:-}" ]]; then
    run_test gtk4-target-selection \
      cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
        --test harness_gtk4_test -- \
        --ignored --nocapture --test-threads=1
  else
    # The GTK4 selection fixture is explicitly X11-only. Native Sway runs
    # headless pixman and cannot initialize its EGL renderer, so record the
    # typed environment limitation instead of manufacturing a pass/failure.
    limitation="X11-only GTK4 fixture is not run in native Wayland/Sway; GTK4 coverage runs in the canonical X11 lane."
    jq -n \
      --arg reason "${limitation}" \
      '{
        schema: "cua-e2e-limitation-v1",
        platform: "linux",
        display_server: "wayland",
        harness: "gtk4",
        test: "gtk4-target-selection",
        status: "not_applicable",
        reason: $reason
      }' > "${ARTIFACT_DIR}/gtk4-target-selection-limitation.json"
    echo "[LIMITATION] gtk4-target-selection: ${limitation}"
  fi
fi

if [[ "${SUITE}" == capture || "${SUITE}" == all ]]; then
  run_test capture-contract \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test capture_contract_test -- \
      --ignored --nocapture --test-threads=1
  run_test desktop-scope \
    cargo test -p cua-driver "${CARGO_DRIVER_FEATURE_ARGS[@]}" \
      --test desktop_scope_linux_test -- \
      --ignored --nocapture --test-threads=1
fi

if [[ "${SUITE}" == shared || "${SUITE}" == all ]]; then
  run_computer_history_gate
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

OWNED_VIDEOS="$(mktemp)"
jq -r 'select(.evidence.video != null) | .evidence.video' "${RESULTS_FILE}" > "${OWNED_VIDEOS}"
while IFS= read -r -d '' video; do
  relative="${video#${ARTIFACT_DIR}/}"
  if [[ "${relative}" == recordings/environment-preflight-*/recording.mp4 ]]; then
    continue
  fi
  if ! grep -Fxq -- "${relative}" "${OWNED_VIDEOS}"; then
    echo "[VIDEO FAIL] Orphan trajectory has no typed result row: ${relative}" >&2
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
  fi
done < <(find "${RECORDING_ROOT}" -type f -name recording.mp4 -print0)
rm -f "${OWNED_VIDEOS}"

while IFS= read -r -d '' recording_error; do
  echo "[VIDEO FAIL] ${recording_error}" >&2
  cat "${recording_error}" >&2
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
  echo "Linux E2E result validation failed" >&2
  FAILURE_COUNT=$((FAILURE_COUNT + 1))
fi

if [[ "${FAILURE_COUNT}" != 0 ]]; then
  echo "Linux Rust e2e suite had ${FAILURE_COUNT} failing lane(s)" >&2
  exit 1
fi

echo "Linux Rust e2e suite completed: ${SUITE}"
