#!/usr/bin/env bash
# Generate the cua_train SDK from the osgym orchestrator's OpenAPI spec.
#
# Usage:
#   ./scripts/generate.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SDK_DIR="$(dirname "$SCRIPT_DIR")"
OSGYM_DIR="$(cd "$SDK_DIR/../../osgym" && pwd)"

# Bootstrap: re-exec inside nix-shell with a proper Python environment
if [ -z "${IN_NIX_SHELL:-}" ]; then
  exec nix-shell --pure -E '
    let pkgs = import <nixpkgs> {};
        python = pkgs.python312.withPackages (ps: with ps; [
          fastapi pydantic uvicorn aiohttp prometheus-client
          kubernetes pyyaml ruff
        ]);
    in pkgs.mkShell {
      buildInputs = [ python pkgs.openapi-python-client ];
      shellHook = "export IN_NIX_SHELL=1";
    }
  ' --run "bash $0"
fi

echo "==> Step 1: Generate orchestrator OpenAPI spec"
ORCH_SPEC=$(mktemp /tmp/osgym-openapi-XXXXXX.json)
PYTHONPATH="$OSGYM_DIR" \
  OSGYM_POOL_BACKEND=fake \
  OSGYM_ALLOWED_IPS=0.0.0.0/0 \
  python3 -c "
import json
from main import app
print(json.dumps(app.openapi(), indent=2))
" > "$ORCH_SPEC"
echo "   Wrote $ORCH_SPEC ($(wc -l < "$ORCH_SPEC") lines)"

echo "==> Step 2: Merge lane endpoints into parent openapi3.yaml"
python3 "$SCRIPT_DIR/merge_orchestrator_spec.py" \
  --parent "$SDK_DIR/openapi3.yaml" \
  --orchestrator "$ORCH_SPEC" \
  --output "$SDK_DIR/openapi3.yaml"

echo "==> Step 3: Regenerate Python SDK"
cd "$SDK_DIR"
openapi-python-client update --path openapi3.yaml || true  # lint-ignore: error-masking — may fail on first run

echo "==> Step 4: Format"
python3 -m ruff check --fix cua_train/ 2>/dev/null || true  # lint-ignore: error-masking — best-effort autofix
python3 -m ruff format cua_train/

rm -f "$ORCH_SPEC"
echo "==> Done. Review changes with: git diff cyclops-cs/python-sdk/"
