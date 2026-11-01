"""Optional checkpoint helpers.  data_dir already persists without them (the
runtime restores and syncs it); these only make the common pattern short:

    from cua import checkpoint
    last = checkpoint.latest(cluster.data_dir)            # newest ckpt-<step>/ or None
    checkpoint.save(cluster.data_dir, step, {"model.pt": buf_or_path})

`latest(dir, "*_adapters.safetensors")` also finds files a framework writes
with the step in their name (e.g. mlx_lm.lora's 0000150_adapters.safetensors).
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
from pathlib import Path
from typing import Any, Iterable, NamedTuple

__all__ = ["Checkpoint", "latest", "save"]


class Checkpoint(NamedTuple):
    step: int
    path: Path


def _step_of(name: str) -> int | None:
    nums = re.findall(r"\d+", name)
    return int(nums[-1]) if nums else None


def latest(directory: str | os.PathLike, pattern: str = "ckpt-*") -> Checkpoint | None:
    """The entry matching `pattern` with the largest number in its name, or None.
    Half-written entries (*.tmp, *.part) never count."""
    return max(_all(Path(directory), pattern), default=None)


def _all(directory: Path, pattern: str) -> list[Checkpoint]:
    out = []
    for p in directory.glob(pattern):
        if p.name.endswith((".tmp", ".part")):
            continue
        step = _step_of(p.name)
        if step is not None:
            out.append(Checkpoint(step, p))
    return out


def _fsync(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    except OSError:
        pass
    finally:
        os.close(fd)


def save(directory: str | os.PathLike, step: int, files: dict[str, Any] | Iterable[Any],
         keep: int = 3) -> Path:
    """Write ckpt-<step>/ atomically under `directory` and keep the newest `keep`.

    `files` maps name -> source path or bytes (or is a list of paths, kept
    under their basenames).  The files go to ckpt-<step>.tmp/ with a
    manifest.json (sha256 per file), are fsynced, then the dir is renamed, so a
    crash never leaves a half checkpoint that `latest` would pick."""
    root = Path(directory)
    root.mkdir(parents=True, exist_ok=True)
    if not isinstance(files, dict):
        files = {Path(f).name: f for f in files}
    step = int(step)
    for name in files:
        if "/" in name or name in ("", ".", "..", "manifest.json"):
            raise ValueError(f"checkpoint file name must be a plain name: {name!r}")
    final = root / f"ckpt-{step}"
    tmp = root / f"ckpt-{step}.tmp"
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir()
    manifest: dict[str, Any] = {"step": step, "files": {}}
    for name, src in files.items():
        dst = tmp / name
        if isinstance(src, (bytes, bytearray)):
            dst.write_bytes(bytes(src))
        else:
            shutil.copyfile(src, dst)
        _fsync(dst)
        manifest["files"][name] = {"sha256": hashlib.sha256(dst.read_bytes()).hexdigest(),
                                   "size": dst.stat().st_size}
    (tmp / "manifest.json").write_text(json.dumps(manifest, sort_keys=True, indent=1) + "\n")
    _fsync(tmp / "manifest.json")
    _fsync(tmp)
    if final.exists():
        aside = root / f"ckpt-{step}.old.tmp"
        shutil.rmtree(aside, ignore_errors=True)
        os.replace(final, aside)
        os.replace(tmp, final)
        shutil.rmtree(aside, ignore_errors=True)
    else:
        os.replace(tmp, final)
    _fsync(root)
    dirs = sorted(c for c in _all(root, "ckpt-*") if c.path.is_dir())
    for old in dirs[:-keep] if keep > 0 else []:
        shutil.rmtree(old.path, ignore_errors=True)
    return final
