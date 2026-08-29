from __future__ import annotations

import re
import shlex
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parents[4]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci-image-api.yml"
CD_WORKFLOW = REPO_ROOT / ".github/workflows/cd-image-api.yml"


def test_image_api_ci_covers_the_contract_and_generated_artifacts() -> None:
    workflow = yaml.safe_load(CI_WORKFLOW.read_text())
    pull_request_paths = workflow[True]["pull_request"]["paths"]
    push_paths = workflow[True]["push"]["paths"]
    assert "clusters/base/cua-images/**" in pull_request_paths
    assert "libs/python/cua-sandbox/cua_sandbox/image.py" in pull_request_paths
    assert "libs/python/cua-sandbox/cua_sandbox/generated/**" in pull_request_paths
    assert "libs/python/cua-sandbox/schemas/**" in pull_request_paths
    assert "libs/python/cua-sandbox/scripts/generate_image_models.py" in pull_request_paths
    assert "libs/python/cua-sandbox/uv.lock" in pull_request_paths
    assert ".github/workflows/cd-image-api.yml" in pull_request_paths
    assert ".github/workflows/cd-image-api.yml" in push_paths
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

    assert workflow["jobs"]["validate"]["timeout-minutes"] >= 15
    admission = next(
        step
        for step in workflow["jobs"]["validate"]["steps"]
        if step["name"] == "Validate CRD API-server admission"
    )["run"]
    assert "go install sigs.k8s.io/kind@v0.26.0" in commands
    assert "kind create cluster --name cua-image-api" in admission
    assert (
        "kubectl apply --server-side --dry-run=server -f clusters/base/cua-images/crd.yaml"
        in admission
    )


def test_image_api_cd_publishes_only_versioned_artifacts() -> None:
    workflow = yaml.safe_load(CD_WORKFLOW.read_text())
    assert workflow[True]["push"]["tags"] == ["image-api-v*"]
    assert workflow["permissions"] == {"contents": "read", "packages": "write"}
    commands = "\n".join(step.get("run", "") for step in workflow["jobs"]["publish"]["steps"])
    assert "flux_2.9.4_linux_amd64.tar.gz" in commands
    assert "c2c397a52930f52d2005c01d276116b059d062de379386d58e98115380a766a2" in commands
    assert "flux push artifact" in commands


def test_image_api_cd_binds_manual_publication_to_the_release_tag_commit() -> None:
    workflow = yaml.safe_load(CD_WORKFLOW.read_text())
    steps = workflow["jobs"]["publish"]["steps"]
    checkout = next(step for step in steps if step["name"] == "Checkout repository")
    version = next(step for step in steps if step["name"] == "Determine version")["run"]

    assert checkout.get("with", {}).get("fetch-depth") == 0
    assert 'VERSION="${MANUAL_VERSION}"' in version
    assert 'VERSION="${GITHUB_REF_NAME#image-api-v}"' in version
    assert '[[ ! "${VERSION}" =~ ^[0-9]+\\.[0-9]+\\.[0-9]+$ ]]' in version
    assert 'RELEASE_TAG="image-api-v${VERSION}"' in version
    assert 'TAG_COMMIT="$(git rev-list -n 1 "${RELEASE_TAG}")"' in version
    assert 'CHECKED_OUT_COMMIT="$(git rev-parse HEAD)"' in version
    assert 'if [[ "${TAG_COMMIT}" != "${CHECKED_OUT_COMMIT}" ]]; then' in version
    assert 'echo "release_tag=${RELEASE_TAG}" >>"${GITHUB_OUTPUT}"' in version
    assert 'echo "commit=${TAG_COMMIT}" >>"${GITHUB_OUTPUT}"' in version


def test_image_api_cd_authenticates_and_rejects_existing_artifacts_before_push() -> None:
    workflow = yaml.safe_load(CD_WORKFLOW.read_text())
    steps = workflow["jobs"]["publish"]["steps"]
    authenticate = next(step for step in steps if step["name"] == "Authenticate to GHCR")["run"]
    publish = next(step for step in steps if step["name"] == "Publish Image API artifact")["run"]

    assert "docker login ghcr.io" in authenticate
    assert 'echo "${{ github.token }}"' in authenticate
    assert '--username "${{ github.actor }}"' in authenticate
    assert "--password-stdin" in authenticate
    assert 'ARTIFACT_REFERENCE="${ARTIFACT_DESTINATION#oci://}"' in publish
    assert 'docker manifest inspect "${ARTIFACT_REFERENCE}"' in publish
    assert 'echo "Image API artifact already exists: ${ARTIFACT_DESTINATION}" >&2' in publish
    assert publish.index("docker manifest inspect") < publish.index("flux push artifact")


def test_image_api_cd_uses_an_exact_immutable_artifact_destination_and_metadata() -> None:
    workflow = yaml.safe_load(CD_WORKFLOW.read_text())
    publish = next(
        step
        for step in workflow["jobs"]["publish"]["steps"]
        if step["name"] == "Publish Image API artifact"
    )
    commands = publish["run"]
    destination_match = re.search(
        r'^ARTIFACT_DESTINATION="(?P<destination>[^"]+)"$', commands, re.MULTILINE
    )

    assert destination_match is not None
    destination = destination_match.group("destination")
    assert re.fullmatch(r"oci://ghcr\.io/trycua/cua-image-api:v\$\{VERSION\}", destination)

    tokens = shlex.split(commands)
    flux_push = tokens.index("flux")
    assert tokens[flux_push : flux_push + 4] == [
        "flux",
        "push",
        "artifact",
        "${ARTIFACT_DESTINATION}",
    ]
    assert "--path=clusters/base/cua-images" in tokens
    assert "--source=${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}" in tokens
    assert "--revision=${RELEASE_TAG}@sha1:${COMMIT}" in tokens


def test_image_api_cd_serializes_publication_by_version_across_triggers() -> None:
    workflow = yaml.safe_load(CD_WORKFLOW.read_text())

    assert workflow.get("concurrency") == {
        "group": "image-api-${{ github.event_name == 'workflow_dispatch' && format('image-api-v{0}', inputs.version) || github.ref_name }}",
        "cancel-in-progress": False,
    }


def test_image_api_cd_fails_closed_unless_the_manifest_is_confirmed_missing() -> None:
    workflow = yaml.safe_load(CD_WORKFLOW.read_text())
    publish = next(
        step
        for step in workflow["jobs"]["publish"]["steps"]
        if step["name"] == "Publish Image API artifact"
    )["run"]

    assert 'MANIFEST_OUTPUT="$(docker manifest inspect "${ARTIFACT_REFERENCE}" 2>&1)"' in publish
    assert "MANIFEST_STATUS=$?" in publish
    assert 'if [[ "${MANIFEST_STATUS}" -eq 0 ]]; then' in publish
    assert "grep -Eiq 'manifest unknown|no such manifest'" in publish
    assert (
        'echo "could not verify Image API artifact absence: ${ARTIFACT_DESTINATION}" >&2' in publish
    )
    assert publish.index("MANIFEST_STATUS=$?") < publish.index("flux push artifact")


def test_image_api_cd_passes_manual_version_through_the_environment() -> None:
    workflow = yaml.safe_load(CD_WORKFLOW.read_text())
    publish = workflow["jobs"]["publish"]
    version = next(step for step in publish["steps"] if step["name"] == "Determine version")["run"]

    assert publish.get("env", {}).get("MANUAL_VERSION") == "${{ inputs.version }}"
    assert 'VERSION="${MANUAL_VERSION}"' in version
    assert "${{ inputs.version }}" not in version
