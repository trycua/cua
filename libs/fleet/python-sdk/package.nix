# Nix package for the cua-train Python SDK.
#
# Single-step build:
#   1. Import the osgym FastAPI app (fake backend) → dump openapi.json
#   2. Merge lane endpoints into parent openapi3.yaml
#   3. Regenerate api/ + models/ with openapi-python-client
#   4. Format with ruff
#   5. Build the wheel
#
# Usage:
#   nix build .#cua-train-sdk
#   pip install result/*.whl
{
  pkgs,
  lib ? pkgs.lib,
  # Build against a caller-supplied interpreter so consumers (e.g. the
  # cua-temporal-agent worker env) can keep the package on the same Python as
  # the rest of their closure. Defaults to 3.12 for the standalone `nix build`.
  python ? pkgs.python312,
}:

let
  pp = python.pkgs;

  osgymSrc = builtins.path {
    path = ../../osgym;
    name = "osgym-src";
  };

  # Python with deps needed to import the osgym FastAPI app (fake backend)
  # AND to run the merge script.
  genPython = python.withPackages (
    ps: with ps; [
      fastapi
      pydantic
      uvicorn
      aiohttp
      prometheus-client
      kubernetes
      pyyaml
      ruff
    ]
  );

in
pp.buildPythonPackage {
  pname = "cua-train";
  version = "0.1.0";
  pyproject = true;

  src = builtins.path {
    path = ./.;
    name = "cua-train-sdk-src";
    filter =
      path: type:
      let
        base = builtins.baseNameOf path;
      in
      !(
        base == ".venv"
        || base == "__pycache__"
        || base == "dist"
        || base == ".pytest_cache"
        || lib.hasSuffix ".egg-info" base
      );
  };

  build-system = [ pp.hatchling ];

  nativeBuildInputs = [
    genPython
    pkgs.openapi-python-client
  ];

  dependencies = with pp; [
    httpx
    attrs
    python-dateutil
  ];

  preBuild = ''
        echo "==> Generating orchestrator OpenAPI spec"
        PYTHONPATH="${osgymSrc}" \
          OSGYM_POOL_BACKEND=fake \
          OSGYM_ALLOWED_IPS=0.0.0.0/0 \
          ${genPython}/bin/python -c "
    import json, sys, io

    # Suppress all stdout noise from osgym imports (log messages, banners)
    real_stdout = sys.stdout
    sys.stdout = io.StringIO()
    try:
        # Lanes were decoupled out of the orchestrator (osgym #4953), so the
        # old \`from lane_endpoints import register\` no longer exists. The lane
        # paths now live in the committed openapi3.yaml (the merge below only
        # ADDS osgym paths, never drops the parent's), so plain app.openapi()
        # is the correct spec source.
        from main import app
        spec = app.openapi()
    finally:
        sys.stdout = real_stdout

    json.dump(spec, sys.stdout, indent=2)
        " > /tmp/orch-spec.json
        echo "   Spec: $(wc -l < /tmp/orch-spec.json) lines"
        test -s /tmp/orch-spec.json || { echo "FATAL: empty spec"; exit 1; }

        echo "==> Merging lane endpoints"
        cat /tmp/orch-spec.json | head -3
        ${genPython}/bin/python scripts/merge_orchestrator_spec.py \
          --parent openapi3.yaml \
          --orchestrator /tmp/orch-spec.json \
          --output openapi3.yaml

        echo "==> Regenerating SDK"
        openapi-python-client update --path openapi3.yaml || true  # lint-ignore: error-masking — may fail on first run

        echo "==> Formatting"
        ${genPython}/bin/python -m ruff check --fix cua_train/ 2>/dev/null || true  # lint-ignore: error-masking — best-effort autofix
        ${genPython}/bin/python -m ruff format cua_train/
  '';

  nativeCheckInputs = with pp; [
    pytestCheckHook
    pytest-httpx
  ];

  pythonImportsCheck = [ "cua_train" ];

  meta = {
    description = "Python SDK for the CUA training / batch backend API";
    license = lib.licenses.mit;
  };
}
