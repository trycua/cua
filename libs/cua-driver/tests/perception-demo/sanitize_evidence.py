#!/usr/bin/env python3
"""Create the only review-safe artifact pair for the authorized demo lane."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil


SHA40 = re.compile(r"^[0-9a-f]{40}$")
SHA64 = re.compile(r"^[0-9a-f]{64}$")
MODEL_ID = re.compile(r"^[A-Za-z0-9._/-]{1,96}$")
ALLOWED_INPUT_KEYS = {"source_sha", "platform", "fixture", "runtime", "result"}


def require_exact_keys(value: dict, expected: set[str], name: str) -> None:
    if set(value) != expected:
        raise ValueError(f"{name} keys must be exactly {sorted(expected)}")


def sanitize(raw: dict, video_path: Path) -> dict:
    require_exact_keys(raw, ALLOWED_INPUT_KEYS, "input")
    require_exact_keys(raw["fixture"], {"id", "oracle"}, "fixture")
    require_exact_keys(raw["fixture"]["oracle"], {"selected", "action_count"}, "fixture.oracle")
    require_exact_keys(raw["runtime"], {"adapter", "model_id", "signed_extension_sha256"}, "runtime")
    require_exact_keys(raw["result"], {"status"}, "result")

    source_sha = raw["source_sha"]
    platform = raw["platform"]
    selected = raw["fixture"]["oracle"]["selected"]
    action_count = raw["fixture"]["oracle"]["action_count"]
    model_id = raw["runtime"]["model_id"]
    extension_sha = raw["runtime"]["signed_extension_sha256"]
    if not isinstance(source_sha, str) or not SHA40.fullmatch(source_sha):
        raise ValueError("source_sha must be 40 lowercase hexadecimal characters")
    if platform not in {"windows", "linux-x11"}:
        raise ValueError("platform must be windows or linux-x11")
    if raw["fixture"]["id"] != "visual-only-canvas/v1":
        raise ValueError("unexpected fixture id")
    if selected not in {"ember", "tide", "moss"}:
        raise ValueError("unexpected oracle selection")
    if type(action_count) is not int or not 1 <= action_count <= 8:
        raise ValueError("action_count must be an integer from 1 through 8")
    if raw["runtime"]["adapter"] != "jev-use":
        raise ValueError("unexpected live adapter")
    if not isinstance(model_id, str) or not MODEL_ID.fullmatch(model_id):
        raise ValueError("model_id contains unsafe characters")
    if not isinstance(extension_sha, str) or not SHA64.fullmatch(extension_sha):
        raise ValueError("signed_extension_sha256 must be 64 lowercase hexadecimal characters")
    if raw["result"]["status"] != "passed":
        raise ValueError("only passing demo evidence may be published")

    size = video_path.stat().st_size
    if not 0 < size <= 100 * 1024 * 1024:
        raise ValueError("recording.mp4 must be nonempty and at most 100 MiB")
    hasher = hashlib.sha256()
    with video_path.open("rb") as video:
        for chunk in iter(lambda: video.read(1024 * 1024), b""):
            hasher.update(chunk)
    digest = hasher.hexdigest()
    return {
        "schema": "cua-visual-perception-demo-evidence/v1",
        "source_sha": source_sha,
        "platform": platform,
        "fixture": {"id": "visual-only-canvas/v1", "oracle": {"selected": selected, "action_count": action_count}},
        "runtime": {"adapter": "jev-use", "model_id": model_id, "signed_extension_sha256": extension_sha},
        "result": {"status": "passed"},
        "artifacts": [{"kind": "video", "path": "recording.mp4", "sha256": digest, "size_bytes": size}],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--recording", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    raw = json.loads(args.input.read_text(encoding="utf-8"))
    manifest = sanitize(raw, args.recording)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(args.recording, args.output_dir / "recording.mp4")
    (args.output_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
