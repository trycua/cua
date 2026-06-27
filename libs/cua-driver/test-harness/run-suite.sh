#!/usr/bin/env bash
# Run the full cua-driver interactive #[ignore] modality/harness matrix for the
# HOST OS (macOS or Linux) in one shot, with a PASS/FAIL/SKIP summary.
#
# Each test file is `#![cfg(target_os = "…")]`-gated, so the non-host files
# compile to empty binaries (0 tests) and are skipped for free — this script
# lists the whole matrix and lets cfg pick the host's subset.
#
# Usage:
#   ./run-suite.sh                 # build the host harness, run the matrix
#   ./run-suite.sh --no-build      # skip the harness build (reuse what's staged)
#   ./run-suite.sh --release       # use the release driver binary (default: debug)
#   CARGO_TEST_FILTER=foo ./run-suite.sh   # only tests whose name contains foo
#
# Windows has its own runner: run-suite.ps1.
set -uo pipefail

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$(cd "$HARNESS_DIR/../rust" && pwd)"
BUILD=1
PROFILE=""
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    --release)  PROFILE="--release" ;;
    *) echo "unknown arg: $arg" >&2; exit 2 ;;
  esac
done

case "$(uname -s)" in
  Darwin) HOST=macos ;;
  Linux)  HOST=linux ;;
  *) echo "run-suite.sh supports macOS + Linux; use run-suite.ps1 on Windows." >&2; exit 2 ;;
esac

# The full matrix (cfg-gated; only the host's run). Grouped by family for a
# readable run order.
TESTS=(
  # harness_ (per-toolkit AX/UIA/AT-SPI/CDP)
  harness_appkit_test harness_swiftui_test           # macOS
  harness_gtk3_test                                  # Linux
  harness_wpf_test harness_winui3_test harness_web_test harness_libreoffice_test  # Windows
  # modality_ (the matrix axes)
  modality_capture_mode_test                         # all OSes
  modality_background_test modality_input_e2e_test   # Windows
  modality_desktop_scope_test                        # Windows desktop scope
  modality_desktop_scope_macos_test                  # macOS desktop scope
  modality_desktop_scope_linux_test                  # Linux desktop scope
  modality_focus_test                                # macOS no-focus-steal
  # guard_
  guard_ux_test                                      # Windows UX guards
)

echo "==> cua-driver interactive matrix — host: $HOST${PROFILE:+ (release)}"

if [ "$BUILD" -eq 1 ]; then
  echo "==> building host harness ($HOST)…"
  bash "$HARNESS_DIR/build/$HOST.sh"
fi

cd "$RUST_DIR"
declare -a SUMMARY
overall=0
for t in "${TESTS[@]}"; do
  out=$(cargo test $PROFILE -p cua-driver --test "$t" \
        ${CARGO_TEST_FILTER:-} -- --ignored --nocapture --test-threads=1 2>&1)
  code=$?
  echo "$out"
  # Parse the last `test result:` line for this file.
  line=$(printf '%s\n' "$out" | grep -E '^test result:' | tail -1)
  passed=$(printf '%s' "$line" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' || echo 0)
  failed=$(printf '%s' "$line" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' || echo 0)
  if [ -z "$line" ]; then
    SUMMARY+=("SKIP  $t (no host tests / did not build)")
  elif [ "${failed:-0}" -gt 0 ] || [ "$code" -ne 0 ]; then
    SUMMARY+=("FAIL  $t (${passed:-0} passed, ${failed:-0} failed)")
    overall=1
  elif [ "${passed:-0}" -eq 0 ]; then
    SUMMARY+=("SKIP  $t (0 tests — not this OS)")
  else
    SUMMARY+=("PASS  $t (${passed} passed)")
  fi
done

echo
echo "================ cua-driver matrix summary ($HOST) ================"
printf '%s\n' "${SUMMARY[@]}"
echo "==================================================================="
exit $overall
