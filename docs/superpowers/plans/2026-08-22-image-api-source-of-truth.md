# Image API Source Of Truth Implementation Plan

<!-- markdownlint-disable MD013 MD032 -->

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Cua Image CRD the canonical API contract, generate JSON Schema and Pydantic v2 models from it, and have `Image.to_build_recipe(...)` return a validated Image custom-resource manifest.

**Architecture:** `trycua/cua` owns a namespaced `images.cua.ai/v1alpha1` CRD and publishes it as a versioned Flux OCI artifact. A deterministic repository script extracts the CRD OpenAPI schema, adds the standard writable Kubernetes resource envelope, generates Pydantic models with `datamodel-code-generator`, and checks generated artifacts into `cua-sandbox`. The existing immutable `Image` builder translates its state into the generated root model and serializes the validated model; `trycua/cloud` later pins and deploys the OCI artifact in a separate pull request.

**Tech Stack:** Kubernetes CRD structural OpenAPI v3 schema, Kustomize 5.8.1, Python 3.11-3.13, Pydantic v2, PyYAML, `datamodel-code-generator` 0.74.0, pytest, GitHub Actions, Flux CLI 2.9.4, GHCR.

## Global Constraints

- `clusters/base/cua-images/crd.yaml` is the only canonical definition of custom Image API fields.
- The API group, version, and kind are exactly `images.cua.ai/v1alpha1` and `Image`.
- The resource is namespaced and enables the status subresource.
- The first version accepts Linux VM recipes only.
- Large file contents and local client paths never appear in the custom resource.
- `Image.to_dict()` output and existing local-runtime behavior remain unchanged.
- Existing Fleet `containerDiskImage` requests remain unchanged and remain the default.
- Generated JSON Schema and Pydantic files are checked in and CI must fail on drift.
- `Image.to_build_recipe(...)` validates with the generated Pydantic root model before returning a dictionary.
- `Image.to_build_recipe(...)` never emits `status`.
- Production must consume an immutable OCI digest, never the moving Cua `main` branch.
- Do not modify `libs/fleet/` in this plan.
- Use test-driven development: every behavior change begins with a focused failing test.

---

### Task 1: Add The Canonical Image CRD

**Files:**
- Create: `clusters/base/cua-images/crd.yaml`
- Create: `clusters/base/cua-images/kustomization.yaml`
- Create: `libs/python/cua-sandbox/tests/test_image_crd.py`
- Modify: `libs/python/cua-sandbox/pyproject.toml`
- Modify: `libs/python/cua-sandbox/uv.lock`

**Interfaces:**
- Consumes: the approved Image resource shape from `docs/superpowers/specs/2026-08-22-image-api-source-of-truth-design.md`.
- Produces: one namespaced `images.cua.ai/v1alpha1` `Image` CRD with structural validation, status, printer columns, and a renderable Kustomize package.

- [ ] **Step 1: Add the YAML test dependency**

Add PyYAML to the `dev` dependency group in `libs/python/cua-sandbox/pyproject.toml`:

```toml
[dependency-groups]
dev = [
    "cua-agent>=0.8.0",
    "PyYAML>=6.0,<7.0",
    "pytest>=9.0.2",
    "pytest-asyncio>=1.3.0",
]
```

Refresh the lockfile:

```bash
uv lock --project libs/python/cua-sandbox
```

- [ ] **Step 2: Write the failing CRD contract tests**

Create `libs/python/cua-sandbox/tests/test_image_crd.py`:

```python
from __future__ import annotations

from pathlib import Path

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
    file_item = schema["properties"]["spec"]["properties"]["recipe"]["properties"][
        "files"
    ]["items"]
    assert file_item["required"] == ["source", "destination"]
    assert file_item["properties"]["source"]["required"] == [
        "reference",
        "digest",
        "sizeBytes",
    ]
    assert "content" not in file_item["properties"]
    assert "path" not in file_item["properties"]["source"]["properties"]


def test_image_crd_has_controller_owned_status_contract() -> None:
    schema = _version()["schema"]["openAPIV3Schema"]
    status = schema["properties"]["status"]
    assert status["properties"]["observedGeneration"]["format"] == "int64"
    assert status["properties"]["recipeDigest"]["pattern"] == "^sha256:[0-9a-f]{64}$"
    assert status["properties"]["artifacts"]["properties"]["oci"]["required"] == [
        "reference",
        "digest",
    ]
    assert status["properties"]["artifacts"]["properties"]["volumeSnapshot"][
        "required"
    ] == ["namespace", "name"]


def test_image_crd_kustomization_contains_only_the_crd() -> None:
    kustomization = yaml.safe_load(KUSTOMIZATION_PATH.read_text())
    assert kustomization == {
        "apiVersion": "kustomize.config.k8s.io/v1beta1",
        "kind": "Kustomization",
        "resources": ["crd.yaml"],
    }
```

- [ ] **Step 3: Run the CRD tests and verify they fail**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_crd.py -q
```

Expected: FAIL because `clusters/base/cua-images/crd.yaml` does not exist.

- [ ] **Step 4: Create the Kustomize package**

Create `clusters/base/cua-images/kustomization.yaml`:

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - crd.yaml
```

- [ ] **Step 5: Create the canonical CRD**

Create `clusters/base/cua-images/crd.yaml` with this structure and exact field contract:

```yaml
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: images.images.cua.ai
spec:
  group: images.cua.ai
  names:
    kind: Image
    listKind: ImageList
    plural: images
    shortNames:
      - cuaimg
    singular: image
  scope: Namespaced
  versions:
    - name: v1alpha1
      served: true
      storage: true
      additionalPrinterColumns:
        - name: Phase
          type: string
          jsonPath: .status.phase
        - name: Ready
          type: string
          jsonPath: .status.conditions[?(@.type=="Ready")].status
        - name: Generation
          type: integer
          format: int64
          jsonPath: .status.observedGeneration
        - name: Age
          type: date
          jsonPath: .metadata.creationTimestamp
      subresources:
        status: {}
      schema:
        openAPIV3Schema:
          title: ImageResource
          type: object
          required:
            - spec
          properties:
            apiVersion:
              type: string
              enum:
                - images.cua.ai/v1alpha1
            kind:
              type: string
              enum:
                - Image
            metadata:
              type: object
            spec:
              title: ImageSpec
              type: object
              additionalProperties: false
              required:
                - recipe
              properties:
                recipe:
                  title: ImageRecipe
                  type: object
                  additionalProperties: false
                  required:
                    - osType
                    - distro
                    - version
                    - kind
                    - layers
                  properties:
                    osType:
                      type: string
                      enum:
                        - linux
                    distro:
                      type: string
                      minLength: 1
                      maxLength: 64
                    version:
                      type: string
                      minLength: 1
                      maxLength: 64
                    kind:
                      type: string
                      enum:
                        - vm
                    layers:
                      type: array
                      maxItems: 128
                      items:
                        title: ImageLayer
                        type: object
                        additionalProperties: false
                        required:
                          - type
                        properties:
                          type:
                            type: string
                            enum:
                              - apt_install
                              - pip_install
                              - uv_install
                              - app_install
                              - run
                          packages:
                            type: array
                            minItems: 1
                            maxItems: 256
                            items:
                              type: string
                              minLength: 1
                              maxLength: 256
                          appId:
                            type: string
                            minLength: 1
                            maxLength: 256
                          command:
                            type: string
                            minLength: 1
                            maxLength: 16384
                        oneOf:
                          - properties:
                              type:
                                enum:
                                  - apt_install
                                  - pip_install
                                  - uv_install
                            required:
                              - packages
                          - properties:
                              type:
                                enum:
                                  - app_install
                            required:
                              - appId
                          - properties:
                              type:
                                enum:
                                  - run
                            required:
                              - command
                    env:
                      type: object
                      maxProperties: 128
                      additionalProperties:
                        type: string
                        maxLength: 8192
                    ports:
                      type: array
                      maxItems: 32
                      uniqueItems: true
                      items:
                        type: integer
                        minimum: 1
                        maximum: 65535
                    files:
                      type: array
                      maxItems: 128
                      items:
                        title: ImageFile
                        type: object
                        additionalProperties: false
                        required:
                          - source
                          - destination
                        properties:
                          source:
                            title: ImageFileReference
                            type: object
                            additionalProperties: false
                            required:
                              - reference
                              - digest
                              - sizeBytes
                            properties:
                              reference:
                                type: string
                                minLength: 1
                                maxLength: 2048
                              digest:
                                type: string
                                pattern: ^sha256:[0-9a-f]{64}$
                              sizeBytes:
                                type: integer
                                format: int64
                                minimum: 0
                          destination:
                            type: string
                            pattern: ^/.*
                            maxLength: 4096
                metadata:
                  title: ImageUserMetadata
                  type: object
                  additionalProperties: false
                  properties:
                    tags:
                      type: object
                      maxProperties: 32
                      additionalProperties:
                        type: string
                        maxLength: 256
                build:
                  title: ImageBuildOptions
                  type: object
                  additionalProperties: false
                  properties:
                    timeoutSeconds:
                      type: integer
                      minimum: 60
                      maximum: 86400
                      default: 7200
                    diskSize:
                      type: string
                      pattern: ^[1-9][0-9]*(Gi|Ti)$
                      default: 40Gi
            status:
              title: ImageStatus
              type: object
              additionalProperties: false
              properties:
                observedGeneration:
                  type: integer
                  format: int64
                  minimum: 0
                phase:
                  type: string
                  enum:
                    - Pending
                    - Validating
                    - Building
                    - PushingOCI
                    - Importing
                    - Quiescing
                    - Snapshotting
                    - Ready
                    - Failed
                    - Cancelling
                buildIdentity:
                  type: string
                  maxLength: 63
                recipeDigest:
                  type: string
                  pattern: ^sha256:[0-9a-f]{64}$
                artifacts:
                  title: ImageArtifacts
                  type: object
                  additionalProperties: false
                  properties:
                    oci:
                      title: ImageOciArtifact
                      type: object
                      additionalProperties: false
                      required:
                        - reference
                        - digest
                      properties:
                        reference:
                          type: string
                          minLength: 1
                          maxLength: 2048
                        digest:
                          type: string
                          pattern: ^sha256:[0-9a-f]{64}$
                    volumeSnapshot:
                      title: ImageVolumeSnapshotArtifact
                      type: object
                      additionalProperties: false
                      required:
                        - namespace
                        - name
                      properties:
                        namespace:
                          type: string
                          minLength: 1
                          maxLength: 63
                        name:
                          type: string
                          minLength: 1
                          maxLength: 253
                logs:
                  title: ImageLogs
                  type: object
                  additionalProperties: false
                  properties:
                    livePodName:
                      type: string
                      maxLength: 253
                    retainedReference:
                      type: string
                      maxLength: 2048
                conditions:
                  type: array
                  x-kubernetes-list-type: map
                  x-kubernetes-list-map-keys:
                    - type
                  items:
                    title: ImageCondition
                    type: object
                    additionalProperties: false
                    required:
                      - type
                      - status
                      - reason
                      - lastTransitionTime
                    properties:
                      type:
                        type: string
                        minLength: 1
                        maxLength: 64
                      status:
                        type: string
                        enum:
                          - "True"
                          - "False"
                          - Unknown
                      reason:
                        type: string
                        minLength: 1
                        maxLength: 128
                      message:
                        type: string
                        maxLength: 32768
                      observedGeneration:
                        type: integer
                        format: int64
                        minimum: 0
                      lastTransitionTime:
                        type: string
                        format: date-time
```

- [ ] **Step 6: Run the focused tests and render the package**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_crd.py -q
go run sigs.k8s.io/kustomize/kustomize/v5@v5.8.1 \
  build clusters/base/cua-images >/tmp/cua-image-crd.yaml
```

Expected: the CRD tests PASS and `/tmp/cua-image-crd.yaml` contains exactly one `CustomResourceDefinition` named `images.images.cua.ai`.

- [ ] **Step 7: Commit the canonical API contract**

```bash
git add clusters/base/cua-images \
  libs/python/cua-sandbox/pyproject.toml \
  libs/python/cua-sandbox/tests/test_image_crd.py \
  libs/python/cua-sandbox/uv.lock
git commit -m "feat(sandbox): define Image custom resource"
```

---

### Task 2: Generate JSON Schema And Pydantic Models

**Files:**
- Create: `libs/python/cua-sandbox/scripts/generate_image_models.py`
- Create: `libs/python/cua-sandbox/schemas/image-v1alpha1.schema.json`
- Create: `libs/python/cua-sandbox/cua_sandbox/generated/__init__.py`
- Create: `libs/python/cua-sandbox/cua_sandbox/generated/image_models.py`
- Create: `libs/python/cua-sandbox/tests/test_image_model_generation.py`
- Modify: `libs/python/cua-sandbox/pyproject.toml`
- Modify: `libs/python/cua-sandbox/uv.lock`

**Interfaces:**
- Consumes: `clusters/base/cua-images/crd.yaml` and `datamodel-code-generator==0.74.0`.
- Produces: `generate_image_models.py [--check]`, standalone JSON Schema, generated `ImageResource` and `ImageFileReference` Pydantic v2 classes, and a CI-safe drift check.

- [ ] **Step 1: Add runtime and generator dependencies**

Add explicit Pydantic runtime ownership and the pinned generator:

```toml
dependencies = [
    "cua-core>=0.3.0,<0.4.0",
    "cua-auto>=0.1.2",
    "cua-fleet==0.1.14",
    "pydantic>=2.11,<3.0",
    "websockets>=12.0",
    "httpx>=0.27.0",
    "oras>=0.2.40",
    "vncdotool>=1.2.0",
    "paramiko>=5.0.0",
    "grpcio==1.78.0",
    "protobuf==6.33.6",
    "pycdlib>=1.14.0",
]

[dependency-groups]
dev = [
    "cua-agent>=0.8.0",
    "datamodel-code-generator==0.74.0",
    "PyYAML>=6.0,<7.0",
    "pytest>=9.0.2",
    "pytest-asyncio>=1.3.0",
]
```

Run:

```bash
uv lock --project libs/python/cua-sandbox
```

- [ ] **Step 2: Write failing generation and drift tests**

Create `libs/python/cua-sandbox/tests/test_image_model_generation.py`:

```python
from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys

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
```

- [ ] **Step 3: Run the generation tests and verify they fail**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_model_generation.py -q
```

Expected: FAIL during collection because `cua_sandbox.generated.image_models` does not exist.

- [ ] **Step 4: Implement the deterministic generator**

Create `libs/python/cua-sandbox/scripts/generate_image_models.py`:

```python
from __future__ import annotations

import argparse
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from typing import Any

import yaml

REPO_ROOT = Path(__file__).resolve().parents[4]
PACKAGE_ROOT = Path(__file__).resolve().parents[1]
CRD_PATH = REPO_ROOT / "clusters/base/cua-images/crd.yaml"
SCHEMA_PATH = PACKAGE_ROOT / "schemas/image-v1alpha1.schema.json"
MODEL_PATH = PACKAGE_ROOT / "cua_sandbox/generated/image_models.py"
API_VERSION = "images.cua.ai/v1alpha1"
SCHEMA_ID = "https://cua.ai/schemas/images.cua.ai/v1alpha1/image.json"

DNS_LABEL = r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$"


def _load_crd() -> dict[str, Any]:
    documents = list(yaml.safe_load_all(CRD_PATH.read_text()))
    if len(documents) != 1:
        raise ValueError(f"expected one CRD document, found {len(documents)}")
    return documents[0]


def _select_version(crd: dict[str, Any]) -> dict[str, Any]:
    versions = [
        version
        for version in crd["spec"]["versions"]
        if version["name"] == "v1alpha1" and version["served"] and version["storage"]
    ]
    if len(versions) != 1:
        raise ValueError("expected one served storage version named v1alpha1")
    return versions[0]


def _writable_metadata_schema() -> dict[str, Any]:
    string_map = {
        "type": "object",
        "additionalProperties": {"type": "string", "maxLength": 4096},
        "maxProperties": 64,
    }
    return {
        "title": "ImageObjectMeta",
        "type": "object",
        "additionalProperties": False,
        "required": ["name", "namespace"],
        "properties": {
            "name": {"type": "string", "minLength": 1, "maxLength": 63, "pattern": DNS_LABEL},
            "namespace": {
                "type": "string",
                "minLength": 1,
                "maxLength": 63,
                "pattern": DNS_LABEL,
            },
            "labels": deepcopy(string_map),
            "annotations": deepcopy(string_map),
        },
    }


def _strip_kubernetes_extensions(value: Any) -> Any:
    if isinstance(value, list):
        return [_strip_kubernetes_extensions(item) for item in value]
    if not isinstance(value, dict):
        return value
    return {
        key: _strip_kubernetes_extensions(item)
        for key, item in value.items()
        if not key.startswith("x-kubernetes-")
    }


def build_schema() -> dict[str, Any]:
    crd = _load_crd()
    version = _select_version(crd)
    schema = _strip_kubernetes_extensions(deepcopy(version["schema"]["openAPIV3Schema"]))
    schema["$schema"] = "https://json-schema.org/draft/2020-12/schema"
    schema["$id"] = SCHEMA_ID
    schema["title"] = "ImageResource"
    schema.setdefault("required", [])
    for field in ("apiVersion", "kind", "metadata", "spec"):
        if field not in schema["required"]:
            schema["required"].append(field)
    schema["properties"]["apiVersion"] = {"type": "string", "const": API_VERSION}
    schema["properties"]["kind"] = {"type": "string", "const": "Image"}
    schema["properties"]["metadata"] = _writable_metadata_schema()
    return schema


def _render_models(schema_path: Path, output_path: Path) -> None:
    subprocess.run(
        [
            sys.executable,
            "-m",
            "datamodel_code_generator",
            "--input",
            str(schema_path),
            "--input-file-type",
            "jsonschema",
            "--output",
            str(output_path),
            "--output-model-type",
            "pydantic_v2.BaseModel",
            "--target-python-version",
            "3.11",
            "--snake-case-field",
            "--use-standard-collections",
            "--use-union-operator",
            "--use-schema-description",
            "--use-field-description",
            "--disable-timestamp",
        ],
        check=True,
    )


def _source_header() -> str:
    digest = hashlib.sha256(CRD_PATH.read_bytes()).hexdigest()
    return (
        "# Source: clusters/base/cua-images/crd.yaml\n"
        f"# Source SHA-256: {digest}\n"
    )


def _write_or_check(path: Path, content: str, *, check: bool) -> None:
    if check:
        if not path.exists() or path.read_text() != content:
            raise SystemExit(f"generated artifact is stale: {path.relative_to(REPO_ROOT)}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(content)
    temporary.replace(path)


def generate(*, check: bool) -> None:
    schema = build_schema()
    schema_content = json.dumps(schema, indent=2, sort_keys=True) + "\n"
    with tempfile.TemporaryDirectory() as temporary_directory:
        temporary_root = Path(temporary_directory)
        temporary_schema = temporary_root / "image.schema.json"
        temporary_model = temporary_root / "image_models.py"
        temporary_schema.write_text(schema_content)
        _render_models(temporary_schema, temporary_model)
        model_content = _source_header() + temporary_model.read_text()
    _write_or_check(SCHEMA_PATH, schema_content, check=check)
    _write_or_check(MODEL_PATH, model_content, check=check)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    generate(check=arguments.check)


if __name__ == "__main__":
    main()
```

Create `libs/python/cua-sandbox/cua_sandbox/generated/__init__.py`:

```python
"""Generated API models. Regenerate with scripts/generate_image_models.py."""
```

- [ ] **Step 5: Generate the checked-in artifacts**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  python libs/python/cua-sandbox/scripts/generate_image_models.py
uv run --python 3.12 --project libs/python/cua-sandbox \
  python libs/python/cua-sandbox/scripts/generate_image_models.py --check
```

Expected: `schemas/image-v1alpha1.schema.json` and
`cua_sandbox/generated/image_models.py` are created, and the final command exits
successfully without changing either file.

- [ ] **Step 6: Run the focused generation tests**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_model_generation.py -q
```

Expected: PASS.

- [ ] **Step 7: Commit the generation pipeline**

```bash
git add libs/python/cua-sandbox/cua_sandbox/generated \
  libs/python/cua-sandbox/pyproject.toml \
  libs/python/cua-sandbox/schemas \
  libs/python/cua-sandbox/scripts/generate_image_models.py \
  libs/python/cua-sandbox/tests/test_image_model_generation.py \
  libs/python/cua-sandbox/uv.lock
git commit -m "feat(sandbox): generate Image API models from CRD"
```

---

### Task 3: Emit Validated Image Custom Resources

**Files:**
- Create: `libs/python/cua-sandbox/tests/test_image_build_recipe.py`
- Modify: `libs/python/cua-sandbox/cua_sandbox/image.py`
- Modify: `libs/python/cua-sandbox/cua_sandbox/__init__.py`

**Interfaces:**
- Consumes: generated `ImageResource` and `ImageFileReference` Pydantic models.
- Produces: `Image.to_build_recipe(...) -> dict[str, Any]` and public `ImageFileReference` export.

- [ ] **Step 1: Write failing resource-emission tests**

Create `libs/python/cua-sandbox/tests/test_image_build_recipe.py`:

```python
from __future__ import annotations

import hashlib
import json

import pytest
from pydantic import ValidationError

from cua_sandbox import Image, ImageFileReference


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


def test_to_build_recipe_resolves_copied_files_without_emitting_local_paths() -> None:
    resource = Image.linux().copy("./secret.txt", "/opt/input.txt").to_build_recipe(
        name="with-file",
        namespace="tenant-a",
        file_references={"./secret.txt": _reference()},
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
    assert hashlib.sha256(canonical_first.encode()).hexdigest() == hashlib.sha256(
        canonical_second.encode()
    ).hexdigest()


def test_to_dict_remains_the_legacy_local_contract() -> None:
    image = Image.linux().copy("./input.txt", "/opt/input.txt")
    assert image.to_dict()["files"] == [["./input.txt", "/opt/input.txt"]]
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_build_recipe.py -q
```

Expected: FAIL because `ImageFileReference` is not exported and
`Image.to_build_recipe` does not exist.

- [ ] **Step 3: Implement `Image.to_build_recipe(...)` through Pydantic**

Update imports in `libs/python/cua-sandbox/cua_sandbox/image.py`:

```python
from typing import Any, Dict, Mapping, Optional, Tuple

from cua_sandbox.generated.image_models import ImageFileReference, ImageResource
```

Add this method immediately before the existing `to_dict()` method:

```python
    def to_build_recipe(
        self,
        *,
        name: str,
        namespace: str,
        tags: Mapping[str, str] | None = None,
        timeout_seconds: int | None = None,
        disk_size: str | None = None,
        file_references: Mapping[str, ImageFileReference] | None = None,
    ) -> Dict[str, Any]:
        """Return a CRD-validated Image custom-resource manifest."""
        if self.os_type != "linux" or self.kind != "vm":
            raise ValueError("remote builds currently support only Linux VM recipes")
        if self._registry is not None:
            raise ValueError("remote builds do not accept registry-only images")
        if self._disk_path is not None:
            raise ValueError("remote builds do not accept local disk images")
        if self._snapshot_source is not None:
            raise ValueError("remote builds do not accept snapshot source images")

        references = dict(file_references or {})
        required_sources = {source for source, _ in self._files}
        missing_sources = sorted(required_sources - references.keys())
        if missing_sources:
            raise ValueError(f"missing file reference for {missing_sources[0]}")
        unused_sources = sorted(references.keys() - required_sources)
        if unused_sources:
            raise ValueError(f"unused file references: {', '.join(unused_sources)}")

        recipe: Dict[str, Any] = {
            "osType": self.os_type,
            "distro": self.distro,
            "version": self.version,
            "kind": self.kind,
            "layers": list(self._layers),
        }
        if self._env:
            recipe["env"] = dict(self._env)
        if self._ports:
            recipe["ports"] = list(self._ports)
        if self._files:
            recipe["files"] = [
                {
                    "source": references[source].model_dump(
                        by_alias=True,
                        exclude_none=True,
                        mode="json",
                    ),
                    "destination": destination,
                }
                for source, destination in self._files
            ]

        spec: Dict[str, Any] = {"recipe": recipe}
        if tags:
            spec["metadata"] = {"tags": dict(tags)}
        build: Dict[str, Any] = {}
        if timeout_seconds is not None:
            build["timeoutSeconds"] = timeout_seconds
        if disk_size is not None:
            build["diskSize"] = disk_size
        if build:
            spec["build"] = build

        validated = ImageResource.model_validate(
            {
                "apiVersion": "images.cua.ai/v1alpha1",
                "kind": "Image",
                "metadata": {"name": name, "namespace": namespace},
                "spec": spec,
            }
        )
        return validated.model_dump(by_alias=True, exclude_none=True, mode="json")
```

- [ ] **Step 4: Export the generated file-reference model**

Update `libs/python/cua-sandbox/cua_sandbox/__init__.py`:

```python
from cua_sandbox.generated.image_models import ImageFileReference
```

Add `"ImageFileReference"` immediately after `"Image"` in `__all__`.

- [ ] **Step 5: Run the focused and legacy Image tests**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest \
    libs/python/cua-sandbox/tests/test_image_build_recipe.py \
    libs/python/cua-sandbox/tests/test_image.py \
    -q
```

Expected: all new tests PASS and the existing 48 Image tests remain green.

- [ ] **Step 6: Commit custom-resource emission**

```bash
git add libs/python/cua-sandbox/cua_sandbox/__init__.py \
  libs/python/cua-sandbox/cua_sandbox/image.py \
  libs/python/cua-sandbox/tests/test_image_build_recipe.py
git commit -m "feat(sandbox): emit validated Image resources"
```

---

### Task 4: Add Image API CI Gates

**Files:**
- Create: `.github/workflows/ci-image-api.yml`
- Create: `libs/python/cua-sandbox/tests/test_image_api_workflows.py`

**Interfaces:**
- Consumes: CRD package, generation script, generated artifacts, and focused Python tests.
- Produces: a path-filtered required workflow that proves CRD rendering, generated-artifact freshness, model validation, and legacy compatibility.

- [ ] **Step 1: Write the failing workflow contract test**

Create `libs/python/cua-sandbox/tests/test_image_api_workflows.py`:

```python
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
    assert "libs/python/cua-sandbox/scripts/generate_image_models.py" in pull_request_paths
    commands = "\n".join(
        step.get("run", "") for step in workflow["jobs"]["validate"]["steps"]
    )
    assert "generate_image_models.py --check" in commands
    assert "kustomize build clusters/base/cua-images" in commands
    assert "test_image_crd.py" in commands
    assert "test_image_model_generation.py" in commands
    assert "test_image_build_recipe.py" in commands
    assert "test_image.py" in commands
```

- [ ] **Step 2: Run the workflow test and verify it fails**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_api_workflows.py -q
```

Expected: FAIL because `.github/workflows/ci-image-api.yml` does not exist.

- [ ] **Step 3: Add the focused CI workflow**

Create `.github/workflows/ci-image-api.yml`:

```yaml
name: "CI: Image API"

on:
  pull_request:
    paths:
      - "clusters/base/cua-images/**"
      - "libs/python/cua-sandbox/cua_sandbox/image.py"
      - "libs/python/cua-sandbox/cua_sandbox/__init__.py"
      - "libs/python/cua-sandbox/cua_sandbox/generated/**"
      - "libs/python/cua-sandbox/schemas/**"
      - "libs/python/cua-sandbox/scripts/generate_image_models.py"
      - "libs/python/cua-sandbox/tests/test_image*.py"
      - "libs/python/cua-sandbox/pyproject.toml"
      - "libs/python/cua-sandbox/uv.lock"
      - ".github/workflows/ci-image-api.yml"
  workflow_dispatch:

permissions:
  contents: read

jobs:
  validate:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Set up Python
        uses: actions/setup-python@v5
        with:
          python-version: "3.12"

      - name: Set up Go
        uses: actions/setup-go@v5
        with:
          go-version: "1.25.x"

      - name: Install dependencies
        run: |
          pip install uv
          uv sync --project libs/python/cua-sandbox --group dev
          go install sigs.k8s.io/kustomize/kustomize/v5@v5.8.1

      - name: Verify generated Image API artifacts
        run: |
          uv run --project libs/python/cua-sandbox \
            python libs/python/cua-sandbox/scripts/generate_image_models.py --check

      - name: Render the CRD package
        run: |
          "$(go env GOPATH)/bin/kustomize" build clusters/base/cua-images >/tmp/cua-image-crd.yaml
          grep -q '^kind: CustomResourceDefinition$' /tmp/cua-image-crd.yaml
          grep -q 'name: images.images.cua.ai' /tmp/cua-image-crd.yaml

      - name: Run focused tests
        run: |
          uv run --project libs/python/cua-sandbox pytest \
            libs/python/cua-sandbox/tests/test_image_crd.py \
            libs/python/cua-sandbox/tests/test_image_model_generation.py \
            libs/python/cua-sandbox/tests/test_image_build_recipe.py \
            libs/python/cua-sandbox/tests/test_image_api_workflows.py \
            libs/python/cua-sandbox/tests/test_image.py \
            -q

      - name: Check formatting and repository hygiene
        run: |
          uv run --project libs/python/cua-sandbox ruff check \
            libs/python/cua-sandbox/cua_sandbox/image.py \
            libs/python/cua-sandbox/scripts/generate_image_models.py \
            libs/python/cua-sandbox/tests/test_image_crd.py \
            libs/python/cua-sandbox/tests/test_image_model_generation.py \
            libs/python/cua-sandbox/tests/test_image_build_recipe.py \
            libs/python/cua-sandbox/tests/test_image_api_workflows.py
          uv run --project libs/python/cua-sandbox ruff format --check \
            libs/python/cua-sandbox/cua_sandbox/image.py \
            libs/python/cua-sandbox/scripts/generate_image_models.py \
            libs/python/cua-sandbox/tests/test_image_crd.py \
            libs/python/cua-sandbox/tests/test_image_model_generation.py \
            libs/python/cua-sandbox/tests/test_image_build_recipe.py \
            libs/python/cua-sandbox/tests/test_image_api_workflows.py
          git diff --check
```

- [ ] **Step 4: Run the workflow contract and local CI-equivalent commands**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_api_workflows.py -q
uv run --python 3.12 --project libs/python/cua-sandbox \
  python libs/python/cua-sandbox/scripts/generate_image_models.py --check
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest \
    libs/python/cua-sandbox/tests/test_image_crd.py \
    libs/python/cua-sandbox/tests/test_image_model_generation.py \
    libs/python/cua-sandbox/tests/test_image_build_recipe.py \
    libs/python/cua-sandbox/tests/test_image_api_workflows.py \
    libs/python/cua-sandbox/tests/test_image.py \
    -q
```

Expected: all commands PASS.

- [ ] **Step 5: Commit the CI gate**

```bash
git add .github/workflows/ci-image-api.yml \
  libs/python/cua-sandbox/tests/test_image_api_workflows.py
git commit -m "ci: validate the Image API contract"
```

---

### Task 5: Publish The Versioned Flux OCI Artifact

**Files:**
- Create: `.github/workflows/cd-image-api.yml`
- Modify: `libs/python/cua-sandbox/tests/test_image_api_workflows.py`

**Interfaces:**
- Consumes: the validated Kustomize package at `clusters/base/cua-images`, Flux CLI 2.9.4, Git tags named `image-api-vMAJOR.MINOR.PATCH`, and GitHub Packages credentials.
- Produces: immutable Flux OCI artifacts at `ghcr.io/trycua/cua-image-api:vMAJOR.MINOR.PATCH`; cloud will pin the emitted digest in a separate pull request.

- [ ] **Step 1: Extend the workflow contract test for publication safety**

Append to `libs/python/cua-sandbox/tests/test_image_api_workflows.py`:

```python
CD_WORKFLOW = REPO_ROOT / ".github/workflows/cd-image-api.yml"


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
```

- [ ] **Step 2: Run the publication contract test and verify it fails**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_api_workflows.py \
  -k publishes_only_versioned_artifacts -q
```

Expected: FAIL because `.github/workflows/cd-image-api.yml` does not exist.

- [ ] **Step 3: Add the digest-producing publication workflow**

Create `.github/workflows/cd-image-api.yml`:

```yaml
name: "CD: Image API"

on:
  push:
    tags:
      - "image-api-v*"
  workflow_dispatch:
    inputs:
      version:
        description: "Image API version without the v prefix"
        required: true
        type: string

permissions:
  contents: read
  packages: write

jobs:
  publish:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Determine version
        id: version
        shell: bash
        run: |
          set -euo pipefail
          if [[ "${GITHUB_EVENT_NAME}" == "workflow_dispatch" ]]; then
            VERSION="${{ inputs.version }}"
          else
            VERSION="${GITHUB_REF_NAME#image-api-v}"
          fi
          if [[ ! "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            echo "invalid Image API version: ${VERSION}" >&2
            exit 1
          fi
          echo "version=${VERSION}" >>"${GITHUB_OUTPUT}"

      - name: Install Flux CLI 2.9.4
        shell: bash
        run: |
          set -euo pipefail
          curl -fsSLo /tmp/flux.tar.gz \
            https://github.com/fluxcd/flux2/releases/download/v2.9.4/flux_2.9.4_linux_amd64.tar.gz
          echo "c2c397a52930f52d2005c01d276116b059d062de379386d58e98115380a766a2  /tmp/flux.tar.gz" \
            | sha256sum -c -
          tar -xzf /tmp/flux.tar.gz -C /tmp flux
          sudo install -m 0755 /tmp/flux /usr/local/bin/flux
          flux version --client

      - name: Authenticate to GHCR
        shell: bash
        run: |
          echo "${{ github.token }}" | docker login ghcr.io \
            --username "${{ github.actor }}" \
            --password-stdin

      - name: Publish Image API artifact
        env:
          VERSION: ${{ steps.version.outputs.version }}
        shell: bash
        run: |
          set -euo pipefail
          flux push artifact "oci://ghcr.io/trycua/cua-image-api:v${VERSION}" \
            --path=clusters/base/cua-images \
            --source="${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}" \
            --revision="${GITHUB_REF_NAME}@sha1:${GITHUB_SHA}"
```

- [ ] **Step 4: Run the complete workflow contract tests**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests/test_image_api_workflows.py -q
```

Expected: PASS.

- [ ] **Step 5: Commit OCI publication support**

```bash
git add .github/workflows/cd-image-api.yml \
  libs/python/cua-sandbox/tests/test_image_api_workflows.py
git commit -m "ci: publish versioned Image API artifacts"
```

---

### Task 6: Run Full Verification And Prepare The Cua Pull Request

**Files:**
- Verify: `clusters/base/cua-images/crd.yaml`
- Verify: `libs/python/cua-sandbox/cua_sandbox/generated/image_models.py`
- Verify: `libs/python/cua-sandbox/cua_sandbox/image.py`
- Verify: `.github/workflows/ci-image-api.yml`
- Verify: `.github/workflows/cd-image-api.yml`

**Interfaces:**
- Consumes: all prior task outputs.
- Produces: one reviewable Cua pull request candidate that changes no Fleet mirror files and does not deploy production desired state.

- [ ] **Step 1: Verify generation is clean**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  python libs/python/cua-sandbox/scripts/generate_image_models.py --check
```

Expected: exit code 0 and no changed files.

- [ ] **Step 2: Run all focused Image API and legacy Image tests**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox pytest \
  libs/python/cua-sandbox/tests/test_image_crd.py \
  libs/python/cua-sandbox/tests/test_image_model_generation.py \
  libs/python/cua-sandbox/tests/test_image_build_recipe.py \
  libs/python/cua-sandbox/tests/test_image_api_workflows.py \
  libs/python/cua-sandbox/tests/test_image.py \
  -v
```

Expected: all tests PASS.

- [ ] **Step 3: Run the complete `cua-sandbox` unit suite**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox \
  pytest libs/python/cua-sandbox/tests -q
```

Expected: PASS. Do not run live Fleet tests or the cross-platform desktop E2E matrix because this change is a pure API/schema contract with no live runtime enablement.

- [ ] **Step 4: Run formatting, rendering, and hygiene checks**

Run:

```bash
uv run --python 3.12 --project libs/python/cua-sandbox ruff check \
  libs/python/cua-sandbox/cua_sandbox/image.py \
  libs/python/cua-sandbox/scripts/generate_image_models.py \
  libs/python/cua-sandbox/tests/test_image_crd.py \
  libs/python/cua-sandbox/tests/test_image_model_generation.py \
  libs/python/cua-sandbox/tests/test_image_build_recipe.py \
  libs/python/cua-sandbox/tests/test_image_api_workflows.py
uv run --python 3.12 --project libs/python/cua-sandbox ruff format --check \
  libs/python/cua-sandbox/cua_sandbox/image.py \
  libs/python/cua-sandbox/scripts/generate_image_models.py \
  libs/python/cua-sandbox/tests/test_image_crd.py \
  libs/python/cua-sandbox/tests/test_image_model_generation.py \
  libs/python/cua-sandbox/tests/test_image_build_recipe.py \
  libs/python/cua-sandbox/tests/test_image_api_workflows.py
go run sigs.k8s.io/kustomize/kustomize/v5@v5.8.1 \
  build clusters/base/cua-images >/tmp/cua-image-crd.yaml
git diff --check
git status --short
```

Expected: all checks PASS; `git status --short` shows only the intended Image API files before the final commit.

- [ ] **Step 5: Confirm the PR boundary**

Run:

```bash
git diff --name-only origin/main...HEAD
```

Expected: paths are limited to:

```text
.github/workflows/cd-image-api.yml
.github/workflows/ci-image-api.yml
clusters/base/cua-images/crd.yaml
clusters/base/cua-images/kustomization.yaml
libs/python/cua-sandbox/cua_sandbox/__init__.py
libs/python/cua-sandbox/cua_sandbox/generated/__init__.py
libs/python/cua-sandbox/cua_sandbox/generated/image_models.py
libs/python/cua-sandbox/cua_sandbox/image.py
libs/python/cua-sandbox/pyproject.toml
libs/python/cua-sandbox/schemas/image-v1alpha1.schema.json
libs/python/cua-sandbox/scripts/generate_image_models.py
libs/python/cua-sandbox/tests/test_image_api_workflows.py
libs/python/cua-sandbox/tests/test_image_build_recipe.py
libs/python/cua-sandbox/tests/test_image_crd.py
libs/python/cua-sandbox/tests/test_image_model_generation.py
libs/python/cua-sandbox/uv.lock
```

No path under `libs/fleet/` or `trycua/cloud` belongs in this pull request.

- [ ] **Step 6: Prepare the pull request metadata**

Use this title:

```text
feat(sandbox): define the Image API contract
```

Use a body that records:

```text
- Cua owns the Image CRD and generated client schema.
- `Image.to_build_recipe(...)` returns a generated-model-validated custom resource.
- The CRD bundle can be published as a versioned Flux OCI artifact.
- No controller, remote build, Fleet route, cloud deployment, or production mutation is included.
- Cloud consumption will land separately and pin an immutable artifact digest.
```

- [ ] **Step 7: Final commit if verification changed generated files**

Only if formatting or regeneration changed intended files:

```bash
git add clusters/base/cua-images \
  libs/python/cua-sandbox \
  .github/workflows/ci-image-api.yml \
  .github/workflows/cd-image-api.yml \
  libs/python/cua-sandbox/uv.lock
git commit -m "chore(sandbox): finalize Image API artifacts"
```

<!-- markdownlint-enable MD013 MD032 -->
