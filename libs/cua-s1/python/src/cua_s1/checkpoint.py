"""Safe, serialization-only checkpoint helpers for Cua-S1 models."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from collections.abc import Mapping
from pathlib import Path
from typing import Any

import torch

FORMAT_NAME = "cua-s1"
FORMAT_VERSION = 1


def _canonical_json(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _state_signature(tensors: Mapping[str, Any], config: Mapping[str, Any]) -> str:
    digest = hashlib.sha256()
    digest.update(_canonical_json(dict(config)))
    for name in sorted(tensors):
        tensor = tensors[name]
        digest.update(_canonical_json([name, str(tensor.dtype), list(tensor.shape)]))
        raw = tensor.detach().cpu().contiguous().reshape(-1).view(torch.uint8)
        digest.update(raw.numpy().tobytes())
    return digest.hexdigest()


def _temporary_path(target: Path) -> Path:
    descriptor, name = tempfile.mkstemp(prefix=f".{target.name}.", suffix=".tmp", dir=target.parent)
    os.close(descriptor)
    return Path(name)


def resolve_checkpoint_paths(directory_or_path: str | Path) -> tuple[Path, Path]:
    """Return the weights and JSON paths for a checkpoint location."""
    path = Path(directory_or_path).expanduser()
    suffix = path.suffix.lower()
    if suffix in {".pt", ".pth", ".bin", ".pkl", ".pickle"}:
        raise ValueError(
            "legacy pickle-based checkpoints are not supported; convert the model "
            "to a safetensors file plus JSON configuration in a trusted environment"
        )
    if suffix == ".safetensors":
        return path, path.with_suffix(".json")
    if suffix == ".json":
        return path.with_suffix(".safetensors"), path
    if suffix:
        raise ValueError("checkpoint path must be a directory, .safetensors file, or .json file")
    return path / "model.safetensors", path / "config.json"


def save_checkpoint_files(
    directory_or_path: str | Path,
    state_dict: Mapping[str, Any],
    config: Mapping[str, Any],
    metadata: Mapping[str, Any] | None = None,
) -> tuple[Path, Path]:
    """Write tensors with safetensors and non-executable data with JSON."""
    weights_path, config_path = resolve_checkpoint_paths(directory_or_path)
    try:
        from safetensors.torch import save_file
    except ImportError as exc:  # pragma: no cover - depends on optional installation
        raise RuntimeError("saving Cua-S1 checkpoints requires the 'safetensors' package") from exc

    if weights_path.exists() and weights_path.is_dir():
        raise ValueError(f"weights path is a directory: {weights_path}")
    if config_path.exists() and config_path.is_dir():
        raise ValueError(f"config path is a directory: {config_path}")
    weights_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.parent.mkdir(parents=True, exist_ok=True)

    tensors: dict[str, Any] = {}
    for name, tensor in state_dict.items():
        if not isinstance(name, str) or not name:
            raise ValueError("state_dict keys must be non-empty strings")
        if not hasattr(tensor, "detach"):
            raise TypeError(f"state_dict value {name!r} is not a tensor")
        tensors[name] = tensor.detach().cpu().contiguous()
    if not tensors:
        raise ValueError("cannot save an empty state_dict")

    try:
        state_signature = _state_signature(tensors, config)
    except (TypeError, ValueError) as exc:
        raise ValueError("checkpoint config and metadata must be JSON serializable") from exc
    document = {
        "format": FORMAT_NAME,
        "format_version": FORMAT_VERSION,
        "state_signature": state_signature,
        "config": dict(config),
        "metadata": dict(metadata or {}),
    }
    try:
        encoded = json.dumps(document, indent=2, sort_keys=True) + "\n"
    except (TypeError, ValueError) as exc:
        raise ValueError("checkpoint config and metadata must be JSON serializable") from exc

    weights_temporary = _temporary_path(weights_path)
    config_temporary = _temporary_path(config_path)
    try:
        save_file(
            tensors,
            str(weights_temporary),
            metadata={
                "format": FORMAT_NAME,
                "format_version": str(FORMAT_VERSION),
                "state_signature": state_signature,
            },
        )
        os.replace(weights_temporary, weights_path)
        config_temporary.write_text(encoded, encoding="utf-8")
        os.replace(config_temporary, config_path)
    finally:
        weights_temporary.unlink(missing_ok=True)
        config_temporary.unlink(missing_ok=True)
    return weights_path, config_path


def load_checkpoint_files(
    directory_or_path: str | Path,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    """Load a safetensors state dict and validate its JSON configuration."""
    weights_path, config_path = resolve_checkpoint_paths(directory_or_path)
    try:
        from safetensors import safe_open
        from safetensors.torch import load_file
    except ImportError as exc:  # pragma: no cover - depends on optional installation
        raise RuntimeError("loading Cua-S1 checkpoints requires the 'safetensors' package") from exc

    if not weights_path.is_file():
        raise FileNotFoundError(f"checkpoint weights not found: {weights_path}")
    if not config_path.is_file():
        raise FileNotFoundError(f"checkpoint config not found: {config_path}")

    try:
        document = json.loads(config_path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid checkpoint JSON: {config_path}") from exc
    if not isinstance(document, dict):
        raise ValueError("checkpoint JSON must contain an object")
    if document.get("format") != FORMAT_NAME:
        raise ValueError(f"unsupported checkpoint format: {document.get('format')!r}")
    if document.get("format_version") != FORMAT_VERSION:
        raise ValueError(
            f"unsupported checkpoint format version: {document.get('format_version')!r}"
        )
    config = document.get("config")
    metadata = document.get("metadata", {})
    state_signature = document.get("state_signature")
    if not isinstance(config, dict):
        raise ValueError("checkpoint JSON field 'config' must be an object")
    if not isinstance(metadata, dict):
        raise ValueError("checkpoint JSON field 'metadata' must be an object")
    if not isinstance(state_signature, str) or len(state_signature) != 64:
        raise ValueError("checkpoint JSON field 'state_signature' must be a SHA-256 digest")

    try:
        with safe_open(str(weights_path), framework="pt", device="cpu") as handle:
            weights_metadata = handle.metadata() or {}
        if weights_metadata.get("format") != FORMAT_NAME:
            raise ValueError("unsupported safetensors checkpoint format")
        if weights_metadata.get("format_version") != str(FORMAT_VERSION):
            raise ValueError("unsupported safetensors checkpoint format version")
        if weights_metadata.get("state_signature") != state_signature:
            raise ValueError("checkpoint state signature mismatch")
        state_dict = load_file(str(weights_path), device="cpu")
    except ValueError:
        raise
    except Exception as exc:
        raise ValueError(f"invalid safetensors checkpoint: {weights_path}") from exc
    if not state_dict:
        raise ValueError("checkpoint contains no tensors")
    if _state_signature(state_dict, config) != state_signature:
        raise ValueError("checkpoint state signature mismatch")
    return state_dict, config, metadata
