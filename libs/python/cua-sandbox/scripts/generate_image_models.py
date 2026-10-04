from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import tempfile
from copy import deepcopy
from pathlib import Path
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


def _translate_kubernetes_extensions(value: Any) -> Any:
    if isinstance(value, list):
        return [_translate_kubernetes_extensions(item) for item in value]
    if not isinstance(value, dict):
        return value
    translated = {key: _translate_kubernetes_extensions(item) for key, item in value.items()}
    if translated.get("x-kubernetes-list-type") == "set":
        translated["uniqueItems"] = True
    return translated


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


def _derive_strict_object_schemas(value: Any) -> Any:
    if isinstance(value, list):
        return [_derive_strict_object_schemas(item) for item in value]
    if not isinstance(value, dict):
        return value
    strict = {key: _derive_strict_object_schemas(item) for key, item in value.items()}
    if (
        strict.get("type") == "object"
        and "properties" in strict
        and not isinstance(strict.get("additionalProperties"), dict)
    ):
        strict["additionalProperties"] = False
    return strict


def build_schema() -> dict[str, Any]:
    crd = _load_crd()
    version = _select_version(crd)
    schema = _derive_strict_object_schemas(
        _strip_kubernetes_extensions(
            _translate_kubernetes_extensions(deepcopy(version["schema"]["openAPIV3Schema"]))
        )
    )
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


def _format_model(output_path: Path) -> None:
    subprocess.run(
        [
            sys.executable,
            "-m",
            "isort",
            "--settings-path",
            str(REPO_ROOT / "pyproject.toml"),
            str(output_path),
        ],
        check=True,
        cwd=REPO_ROOT,
    )
    subprocess.run(
        [
            sys.executable,
            "-m",
            "black",
            "--quiet",
            "--config",
            str(REPO_ROOT / "pyproject.toml"),
            str(output_path),
        ],
        check=True,
        cwd=REPO_ROOT,
    )


def _schema_validation_imports() -> str:
    return """
import json as _json
from typing import Any as _Any

from jsonschema import Draft202012Validator as _Draft202012Validator
from pydantic import model_validator as _model_validator
"""


def _schema_validation_wrapper(schema_content: str) -> str:
    schema_literal = json.dumps(schema_content)
    return f"""
_GENERATED_IMAGE_SCHEMA = _json.loads({schema_literal})
_GENERATED_IMAGE_SCHEMA_VALIDATOR = _Draft202012Validator(_GENERATED_IMAGE_SCHEMA)
GeneratedImageResource = ImageResource


class ImageResource(GeneratedImageResource):
    @_model_validator(mode="before")
    @classmethod
    def _validate_generated_schema(cls, value: _Any) -> _Any:
        errors = sorted(
            _GENERATED_IMAGE_SCHEMA_VALIDATOR.iter_errors(value),
            key=lambda error: (error.json_path, error.message),
        )
        if errors:
            messages = "; ".join(
                f"{{error.json_path}}: {{error.message}}" for error in errors
            )
            raise ValueError(messages)
        return value
"""


def _source_header() -> str:
    digest = hashlib.sha256(CRD_PATH.read_bytes()).hexdigest()
    return f"# Source: clusters/base/cua-images/crd.yaml\n# Source SHA-256: {digest}\n"


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
        generated_model = temporary_model.read_text()
        future_import = "from __future__ import annotations\n"
        if generated_model.count(future_import) != 1:
            raise ValueError("expected one future annotations import in generated model")
        generated_model = generated_model.replace(
            future_import,
            future_import + _schema_validation_imports(),
            1,
        )
        model_content = _source_header() + generated_model
        if "class Source(BaseModel):" not in model_content:
            raise ValueError("expected datamodel-code-generator to emit Source")
        if "class ImageResource(BaseModel):" not in model_content:
            raise ValueError("expected datamodel-code-generator to emit ImageResource")
        model_content += _schema_validation_wrapper(schema_content)
        model_content += (
            "\n# Stable public name derived from the CRD title.\nImageFileReference = Source\n"
        )
        temporary_model.write_text(model_content)
        _format_model(temporary_model)
        model_content = temporary_model.read_text()
    _write_or_check(SCHEMA_PATH, schema_content, check=check)
    _write_or_check(MODEL_PATH, model_content, check=check)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    generate(check=arguments.check)


if __name__ == "__main__":
    main()
