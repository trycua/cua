from __future__ import annotations

from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parents[4]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci-image-api.yml"


def test_image_api_ci_covers_the_contract_and_generated_artifacts() -> None:
    workflow = yaml.safe_load(CI_WORKFLOW.read_text())
    pull_request_paths = workflow[True]["pull_request"]["paths"]
    assert "clusters/base/cua-images/**" in pull_request_paths
    assert "libs/python/cua-sandbox/cua_sandbox/image.py" in pull_request_paths
    assert "libs/python/cua-sandbox/cua_sandbox/generated/**" in pull_request_paths
    assert "libs/python/cua-sandbox/schemas/**" in pull_request_paths
    assert "libs/python/cua-sandbox/scripts/generate_image_models.py" in pull_request_paths
    assert "libs/python/cua-sandbox/uv.lock" in pull_request_paths
    commands = "\n".join(step.get("run", "") for step in workflow["jobs"]["validate"]["steps"])
    assert "uv sync --project libs/python/cua-sandbox --group dev --extra dev" in commands
    assert "generate_image_models.py --check" in commands
    assert 'export PATH="$(go env GOPATH)/bin:$PATH"' in commands
    assert "kustomize build clusters/base/cua-images" in commands
    assert "test_image_crd.py" in commands
    assert "test_image_model_generation.py" in commands
    assert "test_image_build_recipe.py" in commands
    assert "test_image_api_workflows.py" in commands
    assert "test_image.py" in commands
