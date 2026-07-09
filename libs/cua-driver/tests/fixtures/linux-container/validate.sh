#!/usr/bin/env bash
# Build + test cua-driver on the XFCE container, detached (survives `az
# container exec`). Output goes to /tmp/validate.log.
#   validate.sh run    # launch detached
#   validate.sh log    # print the log
set -uo pipefail
RUST=/opt/cua/libs/cua-driver/rust
LOG=/tmp/validate.log
export PATH=/home/cua/.cargo/bin:$PATH
export CARGO_TERM_COLOR=never

_work() {
  echo "=== validate start $(date -u +%H:%M:%S) ==="
  cd "$RUST" || { echo "no rust dir"; exit 9; }
  echo "HEAD: $(git -C /opt/cua rev-parse --short HEAD)"
  echo "--- cargo build -p platform-linux ---"
  cargo build -p platform-linux 2>&1 | tail -25; echo "build_rc=${PIPESTATUS[0]}"
  echo "--- cargo build -p cua-driver (bin) ---"
  cargo build -p cua-driver 2>&1 | tail -15; echo "bin_rc=${PIPESTATUS[0]}"
  echo "--- cargo test -p platform-linux session_bus ---"
  cargo test -p platform-linux session_bus 2>&1 | tail -18; echo "sbtest_rc=${PIPESTATUS[0]}"
  echo "--- cargo test --test modality_dispatch_linux_test ---"
  cargo test -p cua-driver --test modality_dispatch_linux_test 2>&1 | tail -18; echo "dispatch_rc=${PIPESTATUS[0]}"
  echo "VALIDATE_DONE $(date -u +%H:%M:%S)"
}

case "${1:-run}" in
  run)   : > "$LOG"; setsid bash "$0" _work >>"$LOG" 2>&1 </dev/null & echo "launched pid $! -> $LOG" ;;
  _work) _work ;;
  log)   cat "$LOG" 2>/dev/null ;;
  *)     echo "usage: validate.sh run|log" ;;
esac
