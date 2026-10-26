#!/usr/bin/env bash
# Install an SDK release in a throwaway environment and print its public
# surface (JSON) on stdout.
#
#   dump.sh daytona|modal|e2b py|ts VERSION|latest
#
# For CI runners and for jobs in a Space (surface_job.py); the package is
# imported or read here, so never run it on a developer machine.
set -euo pipefail
PROVIDER="$1"; LANGUAGE="$2"; VER="$3"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
W="$(mktemp -d "${TMPDIR:-/tmp}/cua-surface.XXXXXX")"
trap 'rm -rf "$W"' EXIT
case "$PROVIDER/$LANGUAGE" in
  daytona/py) pkg=daytona; mods="daytona" ;;
  daytona/ts) pkg=@daytona/sdk ;;
  modal/py)   pkg=modal; mods="modal modal.exception" ;;
  modal/ts)   pkg=modal ;;
  e2b/py)     pkg=e2b; mods="e2b" ;;
  e2b/ts)     pkg=e2b ;;
  *) echo "unknown $PROVIDER/$LANGUAGE" >&2; exit 2 ;;
esac
if [ "$LANGUAGE" = py ]; then
  spec="$pkg"; [ "$VER" = latest ] || spec="$pkg==$VER"
  python3 -m venv "$W/venv"
  "$W/venv/bin/pip" install -q "$spec" >&2
  # shellcheck disable=SC2086
  "$W/venv/bin/python" "$HERE/surface_py.py" $mods
else
  # @types/node is pinned: it otherwise floats with a transitive dependency
  # and changes how Buffer and Error print (the fixtures were recorded on 22).
  spec="$pkg"; [ "$VER" = latest ] || spec="$pkg@$VER"
  (cd "$W" && npm init -y >/dev/null && npm install --ignore-scripts --no-audit --no-fund --silent "$spec" typescript@5 @types/node@22 >&2)
  node "$HERE/surface_ts.mjs" "$W" "$pkg"
fi
