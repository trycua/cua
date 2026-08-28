from __future__ import annotations

import hashlib
import json

import pytest
from cua_sandbox import Image, ImageFileReference
from pydantic import ValidationError


def _reference(character: str = "a", *, size_bytes: int = 12) -> ImageFileReference:
    return ImageFileReference.model_validate(
        {
            "reference": f"uploads/tenant-a/{character}",
            "digest": "sha256:" + character * 64,
            "sizeBytes": size_bytes,
        }
    )


def test_to_build_recipe_returns_a_complete_validated_resource() -> None:
    resource = (
        Image.linux("ubuntu", "24.04")
        .apt_install("curl", "git")
        .pip_install("playwright")
        .run("python -m playwright install chromium")
        .env(APP_ENV="test")
        .expose(8000)
        .to_build_recipe(
            name="browser-test",
            namespace="tenant-a",
            tags={"team": "evals"},
            timeout_seconds=3600,
            disk_size="40Gi",
        )
    )
    assert resource == {
        "apiVersion": "images.cua.ai/v1alpha1",
        "kind": "Image",
        "metadata": {"name": "browser-test", "namespace": "tenant-a"},
        "spec": {
            "recipe": {
                "osType": "linux",
                "distro": "ubuntu",
                "version": "24.04",
                "kind": "vm",
                "layers": [
                    {"type": "apt_install", "packages": ["curl", "git"]},
                    {"type": "pip_install", "packages": ["playwright"]},
                    {"type": "run", "command": "python -m playwright install chromium"},
                ],
                "env": {"APP_ENV": "test"},
                "ports": [8000],
            },
            "metadata": {"tags": {"team": "evals"}},
            "build": {"timeoutSeconds": 3600, "diskSize": "40Gi"},
        },
    }
    assert "status" not in resource


def test_to_build_recipe_normalizes_app_install_to_crd_field_names() -> None:
    image = Image.linux().app_install("playwright")

    assert image.to_dict()["layers"] == [{"type": "app_install", "app_id": "playwright"}]
    assert image.to_build_recipe(name="app-test", namespace="tenant-a")["spec"]["recipe"][
        "layers"
    ] == [{"type": "app_install", "appId": "playwright"}]


def test_to_build_recipe_resolves_copied_files_without_emitting_local_paths() -> None:
    resource = (
        Image.linux()
        .copy("./secret.txt", "/opt/input.txt")
        .to_build_recipe(
            name="with-file",
            namespace="tenant-a",
            file_references={"./secret.txt": _reference()},
        )
    )
    serialized = json.dumps(resource, sort_keys=True, separators=(",", ":"))
    assert "./secret.txt" not in serialized
    assert resource["spec"]["recipe"]["files"] == [
        {
            "source": {
                "reference": "uploads/tenant-a/a",
                "digest": "sha256:" + "a" * 64,
                "sizeBytes": 12,
            },
            "destination": "/opt/input.txt",
        }
    ]


@pytest.mark.parametrize(
    "reference",
    [
        "/tmp/secret.txt",
        "file:///tmp/secret.txt",
        "data:text/plain;base64,c2VjcmV0",
        "uploads/tenant-a/../secret",
    ],
)
def test_to_build_recipe_rejects_non_uploaded_file_references(reference: str) -> None:
    invalid_reference = _reference().model_copy(update={"reference": reference})

    with pytest.raises(ValidationError):
        Image.linux().copy("./input.txt", "/opt/input.txt").to_build_recipe(
            name="with-file",
            namespace="tenant-a",
            file_references={"./input.txt": invalid_reference},
        )


def test_to_build_recipe_rejects_missing_and_unused_file_references() -> None:
    image = Image.linux().copy("./input.txt", "/opt/input.txt")
    with pytest.raises(ValueError, match="missing file reference for ./input.txt"):
        image.to_build_recipe(name="missing", namespace="tenant-a")
    with pytest.raises(ValueError, match="unused file references: ./extra.txt"):
        Image.linux().to_build_recipe(
            name="unused",
            namespace="tenant-a",
            file_references={"./extra.txt": _reference()},
        )


@pytest.mark.parametrize(
    "image, message",
    [
        (Image.windows(), "only Linux VM recipes"),
        (Image.macos(), "only Linux VM recipes"),
        (Image.android(), "only Linux VM recipes"),
        (Image.linux(kind="container"), "only Linux VM recipes"),
        (Image.from_registry("example.invalid/image:tag"), "registry-only images"),
        (Image.from_file("/tmp/example.qcow2", os_type="linux"), "local disk images"),
        (
            Image.linux()._with(_snapshot_source={"snapshotId": "snapshot-123"}),
            "snapshot source images",
        ),
    ],
)
def test_to_build_recipe_rejects_unsupported_image_sources(image: Image, message: str) -> None:
    with pytest.raises(ValueError, match=message):
        image.to_build_recipe(name="unsupported", namespace="tenant-a")


def test_to_build_recipe_uses_generated_validation() -> None:
    with pytest.raises(ValidationError):
        Image.linux().to_build_recipe(name="Invalid_Name", namespace="tenant-a")
    with pytest.raises(ValidationError):
        Image.linux().to_build_recipe(name="demo", namespace="tenant-a", disk_size="40GB")


def test_to_build_recipe_is_deterministic() -> None:
    image = Image.linux().apt_install("curl").env(A="1", B="2")
    first = image.to_build_recipe(name="demo", namespace="tenant-a")
    second = image.to_build_recipe(name="demo", namespace="tenant-a")
    canonical_first = json.dumps(first, sort_keys=True, separators=(",", ":"))
    canonical_second = json.dumps(second, sort_keys=True, separators=(",", ":"))
    assert canonical_first == canonical_second
    assert (
        hashlib.sha256(canonical_first.encode()).hexdigest()
        == hashlib.sha256(canonical_second.encode()).hexdigest()
    )


def test_to_dict_remains_the_legacy_local_contract() -> None:
    image = Image.linux().copy("./input.txt", "/opt/input.txt")
    assert image.to_dict()["files"] == [["./input.txt", "/opt/input.txt"]]
