from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest
from cua_sandbox.generated.image_models import ImageFileReference, ImageResource
from pydantic import ValidationError

PACKAGE_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = PACKAGE_ROOT / "scripts/generate_image_models.py"
SCHEMA = PACKAGE_ROOT / "schemas/image-v1alpha1.schema.json"
MODEL = PACKAGE_ROOT / "cua_sandbox/generated/image_models.py"


def test_generated_artifacts_match_the_canonical_crd() -> None:
    subprocess.run([sys.executable, str(SCRIPT), "--check"], check=True)


def test_generated_model_is_black_formatted() -> None:
    subprocess.run([sys.executable, "-m", "black", "--check", str(MODEL)], check=True)


def test_generated_model_is_ruff_clean() -> None:
    subprocess.run([sys.executable, "-m", "ruff", "check", str(MODEL)], check=True)


def test_generated_schema_identifies_the_image_resource() -> None:
    schema = json.loads(SCHEMA.read_text())
    assert schema["$id"] == "https://cua.ai/schemas/images.cua.ai/v1alpha1/image.json"
    assert schema["title"] == "ImageResource"
    assert schema["properties"]["spec"]["title"] == "ImageSpec"
    assert "status" in schema["properties"]


def test_generated_models_validate_the_resource_and_file_reference() -> None:
    file_reference = ImageFileReference.model_validate(
        {
            "reference": "uploads/tenant-a/sha256-demo",
            "digest": "sha256:" + "a" * 64,
            "sizeBytes": 123,
        }
    )
    resource = ImageResource.model_validate(
        {
            "apiVersion": "images.cua.ai/v1alpha1",
            "kind": "Image",
            "metadata": {"name": "browser-test", "namespace": "tenant-a"},
            "spec": {
                "recipe": {
                    "osType": "linux",
                    "distro": "ubuntu",
                    "version": "24.04",
                    "kind": "vm",
                    "layers": [{"type": "apt_install", "packages": ["curl"]}],
                    "files": [
                        {
                            "source": file_reference.model_dump(by_alias=True, mode="json"),
                            "destination": "/opt/input.txt",
                        }
                    ],
                }
            },
        }
    )
    assert resource.api_version == "images.cua.ai/v1alpha1"
    assert resource.spec.recipe.os_type == "linux"


@pytest.mark.parametrize(
    "reference",
    [
        "/tmp/secret.txt",
        "file:///tmp/secret.txt",
        "data:text/plain;base64,c2VjcmV0",
        "uploads/tenant-a/../secret",
    ],
)
def test_generated_models_reject_non_uploaded_file_references(reference: str) -> None:
    with pytest.raises(ValidationError):
        ImageFileReference.model_validate(
            {
                "reference": reference,
                "digest": "sha256:" + "a" * 64,
                "sizeBytes": 123,
            }
        )


def test_generated_models_reject_unknown_file_reference_fields() -> None:
    with pytest.raises(ValidationError):
        ImageFileReference.model_validate(
            {
                "reference": "uploads/tenant-a/a",
                "digest": "sha256:" + "a" * 64,
                "sizeBytes": 123,
                "path": "/tmp/secret.txt",
            }
        )


@pytest.mark.parametrize(
    "layer",
    [
        {"type": "apt_install", "packages": ["curl"], "appId": "playwright"},
        {"type": "app_install", "appId": "playwright", "command": "echo unexpected"},
        {"type": "run", "command": "echo ok", "packages": ["curl"]},
    ],
)
def test_generated_models_reject_irrelevant_layer_fields(layer: dict[str, object]) -> None:
    with pytest.raises(ValidationError):
        ImageResource.model_validate(
            {
                "apiVersion": "images.cua.ai/v1alpha1",
                "kind": "Image",
                "metadata": {"name": "browser-test", "namespace": "tenant-a"},
                "spec": {
                    "recipe": {
                        "osType": "linux",
                        "distro": "ubuntu",
                        "version": "24.04",
                        "kind": "vm",
                        "layers": [layer],
                    }
                },
            }
        )


def test_generated_models_accept_file_reference_at_kubernetes_int64_maximum() -> None:
    reference = ImageFileReference.model_validate(
        {
            "reference": "uploads/tenant-a/a",
            "digest": "sha256:" + "a" * 64,
            "sizeBytes": 9223372036854775807,
        }
    )
    assert reference.size_bytes == 9223372036854775807


def test_generated_models_reject_file_reference_larger_than_kubernetes_int64() -> None:
    with pytest.raises(ValidationError):
        ImageFileReference.model_validate(
            {
                "reference": "uploads/tenant-a/a",
                "digest": "sha256:" + "a" * 64,
                "sizeBytes": 9223372036854775808,
            }
        )


def test_generated_models_reject_duplicate_ports() -> None:
    with pytest.raises(ValidationError):
        ImageResource.model_validate(
            {
                "apiVersion": "images.cua.ai/v1alpha1",
                "kind": "Image",
                "metadata": {"name": "browser-test", "namespace": "tenant-a"},
                "spec": {
                    "recipe": {
                        "osType": "linux",
                        "distro": "ubuntu",
                        "version": "24.04",
                        "kind": "vm",
                        "layers": [{"type": "apt_install", "packages": ["curl"]}],
                        "ports": [8080, 8080],
                    }
                },
            }
        )
