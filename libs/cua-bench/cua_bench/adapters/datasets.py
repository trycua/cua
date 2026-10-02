"""Pinned benchmark data: download once, verify, cache.

Every adapter names its data by an immutable reference (a git commit, a
Hugging Face revision) plus, when known, the file's sha256. Files land in
``~/.cache/cua-bench/data/<name>/`` (``CUA_BENCH_CACHE`` moves the root).

Gated Hugging Face datasets use ``HF_TOKEN`` from the environment, or the
``hf`` CLI's cached login; neither is ever printed. A dataset this account
has not been granted raises :class:`GatedDatasetError` with the page where
access is requested.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.request
from pathlib import Path
from typing import Optional


class GatedDatasetError(PermissionError):
    """The dataset needs an access request on Hugging Face first."""


def cache_dir(name: str) -> Path:
    base = os.environ.get("CUA_BENCH_CACHE") or os.path.join(
        os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"), "cua-bench"
    )
    path = Path(base) / "data" / name
    path.mkdir(parents=True, exist_ok=True)
    return path


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _verify(path: Path, sha256: Optional[str]) -> None:
    if sha256 and sha256_of(path) != sha256:
        raise RuntimeError(f"{path}: sha256 does not match the pinned {sha256}")


def fetch_url(url: str, dest: Path, sha256: Optional[str] = None,
              headers: Optional[dict] = None, timeout: float = 300) -> Path:
    """``url`` into ``dest`` (skipped when present and matching ``sha256``)."""
    if dest.is_file() and (sha256 is None or sha256_of(dest) == sha256):
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=dest.parent, delete=False) as tmp:
        req = urllib.request.Request(url, headers={"User-Agent": "cua-bench", **(headers or {})})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            shutil.copyfileobj(r, tmp, 1 << 20)
    tmp_path = Path(tmp.name)
    try:
        _verify(tmp_path, sha256)
    except Exception:
        tmp_path.unlink(missing_ok=True)
        raise
    tmp_path.replace(dest)
    return dest


def hf_file(repo: str, filename: str, revision: str, *, sha256: Optional[str] = None,
            name: Optional[str] = None, gated: bool = False) -> Path:
    """One file of a Hugging Face dataset at a pinned ``revision`` (a commit sha)."""
    dest = cache_dir(name or repo.replace("/", "__")) / revision / filename
    if dest.is_file():
        _verify(dest, sha256)
        return dest
    url = f"https://huggingface.co/datasets/{repo}/resolve/{revision}/{filename}"
    page = f"https://huggingface.co/datasets/{repo}"
    token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")
    if token:
        try:
            return fetch_url(url, dest, sha256, headers={"Authorization": f"Bearer {token}"})
        except urllib.error.HTTPError as e:
            if e.code in (401, 403):
                raise GatedDatasetError(
                    f"{repo} is gated: request access at {page}, then retry") from None
            raise
    if gated and shutil.which("hf"):
        # The hf CLI's cached login (never echoed).
        proc = subprocess.run(
            ["hf", "download", repo, filename, "--repo-type", "dataset", "--revision", revision,
             "--local-dir", str(dest.parent.parent / (revision + ".hf"))],
            capture_output=True, text=True,
        )
        if proc.returncode != 0:
            text = proc.stderr + proc.stdout
            if "requires approval" in text or "gated" in text.lower() or "403" in text:
                raise GatedDatasetError(f"{repo} is gated: request access at {page}, then retry")
            raise RuntimeError(f"hf download {repo}/{filename} failed: {text.strip()[-300:]}")
        got = dest.parent.parent / (revision + ".hf") / filename
        dest.parent.mkdir(parents=True, exist_ok=True)
        got.replace(dest)
        _verify(dest, sha256)
        return dest
    if gated:
        raise GatedDatasetError(
            f"{repo} is gated: request access at {page}, then set HF_TOKEN (or `hf auth login`)")
    return fetch_url(url, dest, sha256)
