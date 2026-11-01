#!/usr/bin/env bash
# The Modal SDKs against the gateway over mock Spaces (no VMs).
#
#   tests/compat/modal/run_hermetic.sh            pinned and latest Python SDK, then JS and Go
#   tests/compat/modal/run_hermetic.sh --latest   only the latest Python SDK (the weekly drift run)
#
# PERF_CHECK=1 also runs the overhead guard against perf/baseline.json.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ "${1:-}" = "--latest" ]; then
    exec "$HERE/../run_hermetic.sh" modal modal
fi
"$HERE/../run_hermetic.sh" modal
"$HERE/run_js_go.sh"
