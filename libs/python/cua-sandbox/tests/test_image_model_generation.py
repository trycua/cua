from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys

import pytest
from pydantic import ValidationError

from cua_sandbox.generated.image_models import ImageFileReference, ImageResource

PACKAGE_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = PACKAGE_ROOT / "scripts/generate_image_models.py"
SCHEMA = PACKAGE_ROOT / "schemas/image-v1alpha1.schema.json"


def test_generated_artifacts_match_the_canonical_crd() -> None:
    subprocess.run([sys.executable, str(SCRIPT), "--check"], check=True)


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


def test_generated_models_reject_layer_fields_invalid_under_one_of() -> None:
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
                        "layers": [{"type": "run", "packages": ["curl"]}],
                    }
                },
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
