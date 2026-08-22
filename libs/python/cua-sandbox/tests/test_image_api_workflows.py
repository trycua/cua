from __future__ import annotations

from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parents[4]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci-image-api.yml"
CD_WORKFLOW = REPO_ROOT / ".github/workflows/cd-image-api.yml"


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


def test_image_api_cd_publishes_only_versioned_artifacts() -> None:
    workflow = yaml.safe_load(CD_WORKFLOW.read_text())
    assert workflow[True]["push"]["tags"] == ["image-api-v*"]
    assert workflow["permissions"] == {"contents": "read", "packages": "write"}
    commands = "\n".join(
        step.get("run", "") for step in workflow["jobs"]["publish"]["steps"]
    )
    assert "flux_2.9.4_linux_amd64.tar.gz" in commands
    assert "c2c397a52930f52d2005c01d276116b059d062de379386d58e98115380a766a2" in commands
    assert "flux push artifact" in commands
    assert "ghcr.io/trycua/cua-image-api:v${VERSION}" in commands
    assert ":latest" not in commands
