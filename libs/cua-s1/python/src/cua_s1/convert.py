"""Convert a legacy pickle checkpoint into the safetensors and JSON pair Cua-S1 loads.

``resolve_checkpoint_paths`` refuses ``.pt``, ``.pth``, ``.bin``, ``.pkl``, and
``.pickle`` and tells the caller to convert the artifact instead. This module is
that conversion step. It reads the archive with ``weights_only=True``, so the
source file is restricted to tensors and plain containers and no pickled code
object is executed, then writes the checked format through
``save_checkpoint_files``.
"""

from __future__ import annotations

import argparse
import json
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from .checkpoint import save_checkpoint_files

CONFIG_KEY = "config"
STATE_KEY = "state_dict"


def _require_mapping(value: Any, label: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError(f"checkpoint archive field {label!r} must be a mapping")
    return value


def _json_safe_extras(archive: Mapping[str, Any]) -> dict[str, Any]:
    extras: dict[str, Any] = {}
    for key, value in archive.items():
        if key in (CONFIG_KEY, STATE_KEY):
            continue
        if not isinstance(key, str):
            raise ValueError("checkpoint archive keys must be strings")
        try:
            json.dumps(value)
        except (TypeError, ValueError) as exc:
            raise ValueError(
                f"checkpoint archive field {key!r} is not JSON serializable; "
                "remove it from the archive before converting"
            ) from exc
        extras[key] = value
    return extras


def convert_archive(
    archive: Mapping[str, Any],
    destination: str | Path,
    source_name: str | None = None,
) -> tuple[Path, Path]:
    """Write a loaded archive as a safetensors file plus its JSON configuration."""
    archive = _require_mapping(archive, "archive")
    if CONFIG_KEY not in archive or STATE_KEY not in archive:
        missing = sorted({CONFIG_KEY, STATE_KEY} - set(archive))
        raise ValueError(f"checkpoint archive is missing required fields: {missing}")
    config = _require_mapping(archive[CONFIG_KEY], CONFIG_KEY)
    state_dict = _require_mapping(archive[STATE_KEY], STATE_KEY)
    metadata = _json_safe_extras(archive)
    if source_name is not None:
        metadata["source"] = source_name
    return save_checkpoint_files(destination, state_dict, config, metadata)


def convert_file(source: str | Path, destination: str | Path) -> tuple[Path, Path]:
    """Read a pickle checkpoint without executing it, then write the safe pair."""
    import torch

    path = Path(source).expanduser()
    if not path.is_file():
        raise FileNotFoundError(f"checkpoint archive not found: {path}")
    archive = torch.load(path, map_location="cpu", weights_only=True)
    return convert_archive(archive, destination, source_name=path.name)


def main(argv: list[str] | None = None) -> int:
    """Convert one checkpoint archive from the command line."""
    parser = argparse.ArgumentParser(
        prog="cua-s1-convert",
        description=(
            "Convert a pickle checkpoint archive into the safetensors and JSON pair "
            "that cua_s1.model.load_checkpoint accepts. Only run this on an archive "
            "you trust."
        ),
    )
    parser.add_argument("source", help="path to the .pt/.pth/.bin archive")
    parser.add_argument(
        "destination",
        help="output directory, or an explicit .safetensors path",
    )
    arguments = parser.parse_args(argv)
    weights_path, config_path = convert_file(arguments.source, arguments.destination)
    print(f"wrote {weights_path}")
    print(f"wrote {config_path}")
    return 0


if __name__ == "__main__":  # pragma: no cover - module entry point
    raise SystemExit(main())
