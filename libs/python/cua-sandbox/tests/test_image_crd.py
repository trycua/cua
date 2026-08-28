from __future__ import annotations

from pathlib import Path
from typing import Any

import yaml

REPO_ROOT = Path(__file__).resolve().parents[4]
CRD_PATH = REPO_ROOT / "clusters/base/cua-images/crd.yaml"
KUSTOMIZATION_PATH = REPO_ROOT / "clusters/base/cua-images/kustomization.yaml"


def _crd() -> dict:
    documents = list(yaml.safe_load_all(CRD_PATH.read_text()))
    assert len(documents) == 1
    return documents[0]


def _version() -> dict:
    versions = _crd()["spec"]["versions"]
    assert len(versions) == 1
    return versions[0]


def _schema_nodes(value: Any) -> list[dict[str, Any]]:
    if isinstance(value, list):
        return [node for item in value for node in _schema_nodes(item)]
    if not isinstance(value, dict):
        return []
    return [value, *[node for item in value.values() for node in _schema_nodes(item)]]


def test_image_crd_identity_and_scope() -> None:
    crd = _crd()
    assert crd["apiVersion"] == "apiextensions.k8s.io/v1"
    assert crd["kind"] == "CustomResourceDefinition"
    assert crd["metadata"]["name"] == "images.images.cua.ai"
    assert crd["spec"]["group"] == "images.cua.ai"
    assert crd["spec"]["scope"] == "Namespaced"
    assert crd["spec"]["names"] == {
        "kind": "Image",
        "listKind": "ImageList",
        "plural": "images",
        "shortNames": ["cuaimg"],
        "singular": "image",
    }


def test_image_crd_serves_one_storage_version_with_status() -> None:
    version = _version()
    assert version["name"] == "v1alpha1"
    assert version["served"] is True
    assert version["storage"] is True
    assert version["subresources"] == {"status": {}}
    assert [column["name"] for column in version["additionalPrinterColumns"]] == [
        "Phase",
        "Ready",
        "Generation",
        "Age",
    ]


def test_image_crd_restricts_the_initial_recipe_contract() -> None:
    schema = _version()["schema"]["openAPIV3Schema"]
    recipe = schema["properties"]["spec"]["properties"]["recipe"]
    assert recipe["properties"]["osType"]["enum"] == ["linux"]
    assert recipe["properties"]["kind"]["enum"] == ["vm"]
    assert recipe["properties"]["layers"]["maxItems"] == 128
    assert recipe["properties"]["files"]["maxItems"] == 128
    assert "registry" not in recipe["properties"]
    assert "diskPath" not in recipe["properties"]
    assert "snapshotSource" not in recipe["properties"]


def test_image_crd_files_use_external_references() -> None:
    schema = _version()["schema"]["openAPIV3Schema"]
    file_item = schema["properties"]["spec"]["properties"]["recipe"]["properties"]["files"]["items"]
    assert file_item["required"] == ["source", "destination"]
    assert file_item["properties"]["source"]["required"] == [
        "reference",
        "digest",
        "sizeBytes",
    ]
    reference = file_item["properties"]["source"]["properties"]["reference"]
    assert reference["pattern"] == "^uploads/[a-z0-9]([-a-z0-9]*[a-z0-9])?/[A-Za-z0-9_-]+$"
    assert (
        file_item["properties"]["source"]["properties"]["sizeBytes"]["maximum"]
        == 9223372036854775807
    )
    assert "content" not in file_item["properties"]
    assert "path" not in file_item["properties"]["source"]["properties"]


def test_image_crd_uses_only_kubernetes_valid_structural_keywords() -> None:
    schema = _version()["schema"]["openAPIV3Schema"]
    nodes = _schema_nodes(schema)

    assert all(node.get("additionalProperties") is not False for node in nodes)
    assert all(node.get("uniqueItems") is not True for node in nodes)
    ports = schema["properties"]["spec"]["properties"]["recipe"]["properties"]["ports"]
    assert ports["x-kubernetes-list-type"] == "set"


def test_image_crd_layers_reject_fields_from_other_layer_shapes() -> None:
    layer = _version()["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["recipe"][
        "properties"
    ]["layers"]["items"]
    exclusions = [
        {field for alternative in branch["not"]["anyOf"] for field in alternative["required"]}
        for branch in layer["oneOf"]
    ]

    assert all("additionalProperties" not in branch for branch in layer["oneOf"])
    assert exclusions == [{"appId", "command"}, {"command", "packages"}, {"appId", "packages"}]


def test_image_crd_has_controller_owned_status_contract() -> None:
    schema = _version()["schema"]["openAPIV3Schema"]
    status = schema["properties"]["status"]
    assert status["properties"]["observedGeneration"]["format"] == "int64"
    assert status["properties"]["recipeDigest"]["pattern"] == "^sha256:[0-9a-f]{64}$"
    assert status["properties"]["artifacts"]["properties"]["oci"]["required"] == [
        "reference",
        "digest",
    ]
    assert status["properties"]["artifacts"]["properties"]["volumeSnapshot"]["required"] == [
        "namespace",
        "name",
    ]


def test_image_crd_kustomization_contains_only_the_crd() -> None:
    kustomization = yaml.safe_load(KUSTOMIZATION_PATH.read_text())
    assert kustomization == {
        "apiVersion": "kustomize.config.k8s.io/v1beta1",
        "kind": "Kustomization",
        "resources": ["crd.yaml"],
    }
