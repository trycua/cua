#!/usr/bin/env bash
# macOS lane of verify-cua-driver: runs the canonical Lume guest matrix
# (libs/cua-driver/tests/runners/macos-lume/run-all.sh) through the maintainer
# host's installed lume-e2e wrapper. See ../macos-lume.md.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
LUME_E2E="${LUME_E2E:-$HOME/.local/bin/lume-e2e}"
usage() {
  echo 'usage: verify-macos.sh [--source CLEAN_CHECKOUT] focus <registered-cell> | full [--standalone-browser] | doctor' >&2
  exit 2
}
SOURCE="$ROOT"
if [[ "${1:-}" == --source ]]; then
  [[ $# -ge 2 ]] || usage
  SOURCE="$(cd "$2" && pwd)"
  shift 2
fi
MODE="${1:-}"
shift "$(( $# > 0 ? 1 : 0 ))"
args=()
case "$MODE" in
  focus) [[ $# == 1 ]] || usage; args=(--focus-cell "$1") ;;
  full) [[ $# == 0 || "$*" == --standalone-browser ]] || usage; args=("$@") ;;
  doctor) [[ $# == 0 ]] || usage ;;
  *) usage ;;
esac
[[ "$(uname -s)" == Darwin ]] || { echo 'macOS lane runs on the Lume host only' >&2; exit 2; }
[[ -x "$LUME_E2E" ]] || {
  echo "no lume-e2e wrapper at $LUME_E2E; on another Mac follow libs/cua-driver/tests/runners/macos-lume/README.md by hand" >&2
  exit 2
}
"$LUME_E2E" --doctor
[[ "$MODE" == doctor ]] && exit 0
SHA="$(git -C "$SOURCE" rev-parse HEAD)"
if [[ -n "$(git -C "$SOURCE" status --porcelain --untracked-files=normal)" ]]; then
  echo "source $SOURCE is dirty; commit or use a clean worktree at the SHA under test" >&2
  exit 2
fi
if [[ "$MODE" == focus ]] && ! git -C "$SOURCE" cat-file -e "$SHA:scripts/ci/macos/appkit-cells.tsv" 2>/dev/null \
  && [[ "${args[1]}" == macos-appkit-* ]]; then
  echo "$SHA has no scripts/ci/macos/appkit-cells.tsv, so AppKit cells cannot run focused; use: full" >&2
  exit 2
fi
echo "Source: $SOURCE at $SHA"
# The wrapper prints evidence=<run dir> before cloning and result.json at the end.
exec "$LUME_E2E" --source "$SOURCE" --sha "$SHA" ${args[@]+"${args[@]}"}
