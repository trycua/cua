#!/usr/bin/env python3
"""Cache exact-run encrypted evidence for retryable local validation and rendering.

This tool does not certify a run or decrypt evidence. It pins a GitHub artifact's
ZIP digest and admits only the two encrypted evidence envelopes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile

from evidence_envelope import _rename_directory_no_replace


REPO = "trycua/cua"
MEMBERS = ("primary-desktop.cuae", "window.cuae")
SHA = re.compile(r"[0-9a-f]{40}\Z")
DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")
WORKFLOW = re.compile(r"\.github/workflows/[A-Za-z0-9_.-]+\.yml\Z")
MAX_ARCHIVE = 220 * 1024 * 1024
MAX_ENVELOPE = 101 * 1024 * 1024


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def private_directory(path: Path) -> None:
    if not path.exists() and not path.is_symlink():
        path.mkdir(mode=0o700)
    mode = path.lstat().st_mode
    require(stat.S_ISDIR(mode) and not path.is_symlink(), "cache must be a real directory")
    require(mode & 0o077 == 0, "cache directory must not be accessible to group or others")


def private_file(path: Path, label: str) -> None:
    mode = path.lstat().st_mode
    require(stat.S_ISREG(mode) and not path.is_symlink(), f"{label} must be a regular file")
    require(mode & 0o077 == 0, f"{label} must not be accessible to group or others")


def gh_json(endpoint: str) -> dict:
    result = subprocess.run(["gh", "api", endpoint], check=True, capture_output=True, text=True)
    value = json.loads(result.stdout)
    require(isinstance(value, dict), "GitHub returned invalid metadata")
    return value


def identity(run_id: int, artifact_id: int, source_sha: str, workflow: str, name: str) -> dict:
    require(run_id > 0 and artifact_id > 0, "run and artifact IDs must be positive")
    require(SHA.fullmatch(source_sha) is not None, "source SHA must be 40 lowercase hex characters")
    require(WORKFLOW.fullmatch(workflow) is not None, "workflow must be a repository workflow path")
    require(bool(name) and len(name) <= 128 and re.fullmatch(r"[A-Za-z0-9._-]+", name), "invalid artifact name")
    run = gh_json(f"repos/{REPO}/actions/runs/{run_id}")
    require(run.get("id") == run_id and run.get("head_sha") == source_sha, "run source identity mismatch")
    require((run.get("repository") or {}).get("full_name") == REPO, "run repository mismatch")
    require(run.get("path") == workflow, "run workflow identity mismatch")
    require((run.get("status"), run.get("conclusion")) == ("completed", "success"), "run is not a completed success")
    artifact = gh_json(f"repos/{REPO}/actions/artifacts/{artifact_id}")
    producer = artifact.get("workflow_run") or {}
    require(artifact.get("id") == artifact_id and artifact.get("name") == name, "artifact identity mismatch")
    require(artifact.get("expired") is False, "artifact has expired")
    require(producer.get("id") == run_id and producer.get("head_sha") == source_sha, "artifact producer mismatch")
    require(producer.get("repository_id") == run["repository"].get("id"), "artifact repository mismatch")
    digest = artifact.get("digest")
    require(isinstance(digest, str) and DIGEST.fullmatch(digest) is not None, "artifact has no SHA-256 digest")
    return {
        "schema": "cua-encrypted-evidence-cache/v1",
        "repository": REPO,
        "run_id": run_id,
        "artifact_id": artifact_id,
        "source_sha": source_sha,
        "workflow": workflow,
        "name": name,
        "archive_digest": digest,
    }


def archive_digest(path: Path) -> str:
    private_file(path, "cached archive")
    require(0 < path.stat().st_size <= MAX_ARCHIVE, "cached archive size is invalid")
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def inspect_archive(path: Path) -> None:
    try:
        with zipfile.ZipFile(path) as archive:
            infos = archive.infolist()
            require(sorted(info.filename for info in infos) == list(MEMBERS), "archive must contain only encrypted evidence envelopes")
            for info in infos:
                require(not info.is_dir() and not info.flag_bits & 1, "archive has an unsupported member")
                require(0 < info.file_size <= MAX_ENVELOPE, "encrypted evidence size is invalid")
                require((info.external_attr >> 16) & 0o170000 != stat.S_IFLNK, "archive contains a link")
                with archive.open(info) as handle:
                    require(handle.read(8) == b"CUAEVID\x00", "archive member is not an evidence envelope")
                    while handle.read(1024 * 1024):
                        pass  # Check CRC before caching or staging.
    except (zipfile.BadZipFile, RuntimeError) as error:
        raise ValueError("archive is invalid") from error


def paths(cache: Path, artifact_id: int) -> tuple[Path, Path]:
    return cache / f"artifact-{artifact_id}.zip", cache / f"artifact-{artifact_id}.json"


def read_cached(cache: Path, expected: dict) -> Path:
    archive, metadata = paths(cache, expected["artifact_id"])
    private_file(metadata, "cache metadata")
    require(json.loads(metadata.read_text(encoding="utf-8")) == expected, "cached artifact identity changed")
    require(archive_digest(archive) == expected["archive_digest"], "cached archive digest mismatch")
    inspect_archive(archive)
    return archive


def publish_metadata(cache: Path, metadata: Path, expected: dict) -> None:
    with tempfile.NamedTemporaryFile(dir=cache, prefix=".identity-", mode="w", encoding="utf-8", delete=False) as handle:
        temporary = Path(handle.name)
        try:
            os.chmod(temporary, 0o600)
            json.dump(expected, handle, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
            try:
                os.link(temporary, metadata)
            except FileExistsError:
                # Another invocation may have finished the same immutable artifact.
                private_file(metadata, "cache metadata")
                require(json.loads(metadata.read_text(encoding="utf-8")) == expected, "cached artifact identity changed")
        finally:
            temporary.unlink(missing_ok=True)


def fetch(cache: Path, expected: dict) -> Path:
    private_directory(cache)
    archive, metadata = paths(cache, expected["artifact_id"])
    if metadata.exists() or metadata.is_symlink():
        return read_cached(cache, expected)
    if archive.exists() or archive.is_symlink():
        # Recover a fully published ZIP after an interruption before metadata.
        require(archive_digest(archive) == expected["archive_digest"], "cached archive digest mismatch")
        inspect_archive(archive)
        publish_metadata(cache, metadata, expected)
        return read_cached(cache, expected)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(dir=cache, prefix=".evidence-", delete=False) as handle:
            temporary = Path(handle.name)
            os.chmod(temporary, 0o600)
            with subprocess.Popen(
                ["gh", "api", f"repos/{REPO}/actions/artifacts/{expected['artifact_id']}/zip"],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            ) as process:
                assert process.stdout is not None
                try:
                    for chunk in iter(lambda: process.stdout.read(1024 * 1024), b""):
                        require(handle.tell() + len(chunk) <= MAX_ARCHIVE, "downloaded archive is too large")
                        handle.write(chunk)
                except (OSError, ValueError):
                    process.kill()
                    raise
                require(process.wait() == 0, "artifact download failed")
            handle.flush()
            os.fsync(handle.fileno())
        require(archive_digest(temporary) == expected["archive_digest"], "downloaded archive digest mismatch")
        inspect_archive(temporary)
        try:
            os.link(temporary, archive)  # Do not overwrite a concurrent or earlier download.
        except FileExistsError:
            require(archive_digest(archive) == expected["archive_digest"], "cached archive digest mismatch")
            inspect_archive(archive)
        publish_metadata(cache, metadata, expected)
        return read_cached(cache, expected)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def stage(archive: Path, destination: Path, expected_digest: str) -> None:
    require(not destination.exists() and not destination.is_symlink(), "output directory already exists")
    private_directory(destination.parent)
    require(archive_digest(archive) == expected_digest, "cached archive digest mismatch")
    inspect_archive(archive)
    temporary = Path(tempfile.mkdtemp(dir=destination.parent, prefix=".stage-"))
    try:
        with zipfile.ZipFile(archive) as source:
            for name in MEMBERS:
                target = temporary / name
                with source.open(name) as content, target.open("xb") as output:
                    shutil.copyfileobj(content, output)
                os.chmod(target, 0o600)
        require(archive_digest(archive) == expected_digest, "cached archive changed during staging")
        # Directory rename must not replace an existing (possibly concurrent) stage.
        require(not destination.exists() and not destination.is_symlink(), "output directory already exists")
        _rename_directory_no_replace(temporary, destination)
    finally:
        if temporary.exists():
            for name in MEMBERS:
                (temporary / name).unlink(missing_ok=True)
            temporary.rmdir()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("fetch", "stage"))
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--artifact-id", type=int, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--workflow", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, help="private directory for encrypted envelopes (stage only)")
    args = parser.parse_args()
    require((args.command == "stage") == (args.output is not None), "--output is required only for stage")
    expected = identity(args.run_id, args.artifact_id, args.source_sha, args.workflow, args.name)
    if args.command == "fetch":
        fetch(args.cache, expected)
    else:
        private_directory(args.cache)
        stage(read_cached(args.cache, expected), args.output, expected["archive_digest"])
    print(json.dumps({"status": args.command, "source_sha": args.source_sha, "run_id": args.run_id, "artifact_id": args.artifact_id}))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"evidence cache: {error}", file=sys.stderr)
        raise SystemExit(2)
