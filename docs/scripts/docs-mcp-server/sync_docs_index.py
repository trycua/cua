"""Bounded S3 consumer for the docs MCP paired-generation format.

Reads ``docs_db/current.json`` once, fetches exactly that generation's manifest
and allowlisted files into a staging directory on the destination filesystem,
verifies every hash, installs the immutable generation directory, and only then
replaces the local ``current.json`` atomically. Every failure leaves the last
good pointer, existing generations, flat legacy files, and code data untouched.
"""

import argparse
import fcntl
import json
import logging
import math
import os
import re
import shutil
import stat
import sys
import time
import uuid
from pathlib import Path, PurePosixPath

sys.path.insert(0, str(Path(__file__).resolve().parent))
import docs_index  # noqa: E402

log = logging.getLogger("sync_docs_index")

POINTER_KEY = "docs_db/current.json"
GENERATION_PREFIX = "docs_db/generations/"
BUILD_ID = re.compile(r"[a-f0-9]{32}")
SHA256 = re.compile(r"[a-f0-9]{64}")
MAX_POINTER_BYTES = 4 * 1024
MAX_MANIFEST_BYTES = 16 * 1024 * 1024
LOCK_NAME = ".sync-docs-index.lock"
STAGE_PREFIX = ".stage-"
MISSING_CODES = {"NoSuchKey", "404", "NotFound"}


class SyncError(RuntimeError):
    """Operational failure; last-good state is preserved."""


class MissingPointer(SyncError):
    """The remote ``docs_db/current.json`` object does not exist."""


class LockHeld(SyncError):
    """Another consumer holds the destination lock."""


def _is_missing(error):
    code = getattr(error, "response", None)
    if not isinstance(code, dict):
        return False
    return str(code.get("Error", {}).get("Code")) in MISSING_CODES


def read_json_object(s3, bucket, key, limit):
    """Fetch and parse one small JSON object, never reading more than ``limit`` bytes."""
    try:
        response = s3.get_object(Bucket=bucket, Key=key)
    except Exception as error:
        if _is_missing(error):
            raise MissingPointer(f"Missing object {key}") from error
        raise SyncError(f"Cannot read {key}: {type(error).__name__}") from error
    stream = response["Body"]
    try:
        length = response.get("ContentLength")
        if length is not None and int(length) > limit:
            raise ValueError(f"Object {key} exceeds {limit} bytes")
        body = stream.read(limit + 1)
    finally:
        stream.close()
    if len(body) > limit:
        raise ValueError(f"Object {key} exceeds {limit} bytes")
    try:
        value = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"Object {key} is not valid JSON") from error
    if not isinstance(value, dict):
        raise ValueError(f"Object {key} is not a JSON object")
    return value


def validate_pointer(pointer):
    if not isinstance(pointer, dict) or set(pointer) != {"build_id", "manifest_sha256"}:
        raise ValueError("Invalid docs pointer shape")
    if not isinstance(pointer["build_id"], str) or not BUILD_ID.fullmatch(pointer["build_id"]):
        raise ValueError("Invalid docs build id")
    if not isinstance(pointer["manifest_sha256"], str) or not SHA256.fullmatch(
        pointer["manifest_sha256"]
    ):
        raise ValueError("Invalid docs manifest hash")
    return pointer


def safe_relative_path(name):
    """Accept only ``docs.sqlite`` or plain relative paths beneath ``docs.lance/``."""
    if not isinstance(name, str) or not name or "\\" in name or "\0" in name:
        raise ValueError("Invalid generation file path")
    if name.startswith("/") or name.endswith("/") or "//" in name:
        raise ValueError("Invalid generation file path")
    parts = tuple(name.split("/"))
    if any(part in ("", ".", "..") for part in parts) or PurePosixPath(name).is_absolute():
        raise ValueError("Invalid generation file path")
    if name == "manifest.json" or parts[0] == "manifest.json":
        raise ValueError("Generation manifest collision")
    if name != "docs.sqlite" and not (parts[0] == "docs.lance" and len(parts) > 1):
        raise ValueError("Generation file outside docs.sqlite and docs.lance/")
    return parts


def validate_manifest(manifest, pointer):
    if not isinstance(manifest, dict):
        raise ValueError("Invalid docs manifest")
    if manifest.get("build_id") != pointer["build_id"]:
        raise ValueError("Manifest build id mismatch")
    if docs_index.digest(manifest) != pointer["manifest_sha256"]:
        raise ValueError("Manifest identity mismatch")
    if (
        manifest.get("format_version") != docs_index.FORMAT_VERSION
        or manifest.get("embedding") != docs_index.EMBEDDING
    ):
        raise ValueError("Unsupported docs generation")
    files = manifest.get("files")
    if not isinstance(files, dict) or "docs.sqlite" not in files:
        raise ValueError("Incomplete docs generation manifest")
    for name, sha in files.items():
        safe_relative_path(name)
        if not isinstance(sha, str) or not SHA256.fullmatch(sha):
            raise ValueError(f"Malformed digest for {name}")
    return manifest


def _reject_symlinks(path):
    for entry in [Path(path), *Path(path).rglob("*")]:
        if entry.is_symlink():
            raise ValueError("Symlink inside docs generation")


def verify_existing_generation(target, manifest):
    """An installed generation is immutable: it must already be this exact manifest."""
    if target.is_symlink() or not target.is_dir():
        raise ValueError("Existing generation path is not a directory")
    _reject_symlinks(target)
    manifest_file = target / "manifest.json"
    if not manifest_file.is_file() or manifest_file.stat().st_size > MAX_MANIFEST_BYTES:
        raise ValueError("Existing generation manifest unreadable")
    try:
        installed = json.loads(manifest_file.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("Existing generation manifest corrupt") from error
    if installed != manifest:
        raise ValueError("Existing generation conflicts with remote manifest")
    docs_index.verify_files(target, manifest)


def _read_local_pointer(destination):
    pointer_file = destination / "current.json"
    try:
        if not pointer_file.is_file() or pointer_file.stat().st_size > MAX_POINTER_BYTES:
            return None
        return json.loads(pointer_file.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return None


def _stage_generation(s3, bucket, manifest, destination):
    build_id = manifest["build_id"]
    generations = destination / "generations"
    stage = generations / f"{STAGE_PREFIX}{build_id}-{uuid.uuid4().hex}"
    stage.mkdir()
    try:
        for name in sorted(manifest["files"]):
            parts = safe_relative_path(name)
            file = stage.joinpath(*parts)
            file.parent.mkdir(parents=True, exist_ok=True)
            try:
                s3.download_file(bucket, f"{GENERATION_PREFIX}{build_id}/{name}", str(file))
            except Exception as error:
                raise SyncError(f"Download failed: {type(error).__name__}") from error
        docs_index.write_json(stage / "manifest.json", manifest)
        _reject_symlinks(stage)
        docs_index.verify_files(stage, manifest)
        return stage
    except BaseException:
        # Only this process's own incomplete staging directory is discarded.
        if stage.parent == generations and stage.name.startswith(STAGE_PREFIX):
            shutil.rmtree(stage, ignore_errors=True)
        raise


def _install(stage, target, manifest):
    try:
        stage.rename(target)
    except OSError:
        shutil.rmtree(stage, ignore_errors=True)
        if not target.exists():
            raise
        # A concurrent installer won; accept only an identical generation.
        verify_existing_generation(target, manifest)
        return False
    return True


class destination_lock:
    """Advisory flock on the destination so overlapping consumers serialize."""

    def __init__(self, destination, timeout):
        self.path = Path(destination) / LOCK_NAME
        self.timeout = timeout
        self.fd = None

    def __enter__(self):
        self.fd = os.open(self.path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o644)
        if not stat.S_ISREG(os.fstat(self.fd).st_mode):
            os.close(self.fd)
            self.fd = None
            raise ValueError("Docs sync lock is not a regular file")
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return self
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    os.close(self.fd)
                    self.fd = None
                    raise LockHeld("Another docs sync holds the destination lock") from None
                time.sleep(0.05)

    def __exit__(self, *exc):
        fcntl.flock(self.fd, fcntl.LOCK_UN)
        os.close(self.fd)
        self.fd = None
        return False


def sync(s3, bucket, destination, *, allow_missing_pointer=False, lock_timeout=300.0):
    """Install and activate the generation named by the remote pointer, read once."""
    if not math.isfinite(lock_timeout) or lock_timeout < 0:
        raise ValueError("Lock timeout must be finite and nonnegative")
    if Path(destination).is_symlink():
        raise ValueError("Docs destination is a symlink")
    destination = Path(destination).resolve()
    destination.mkdir(parents=True, exist_ok=True)
    generations = destination / "generations"
    if generations.is_symlink():
        raise ValueError("Docs generations directory is a symlink")
    generations.mkdir(exist_ok=True)
    with destination_lock(destination, lock_timeout):
        local_pointer = destination / "current.json"
        if local_pointer.is_symlink():
            raise ValueError("Docs current pointer is a symlink")
        try:
            pointer = read_json_object(s3, bucket, POINTER_KEY, MAX_POINTER_BYTES)
        except MissingPointer:
            if allow_missing_pointer and not local_pointer.exists():
                log.info("No remote docs pointer; initial migration leaves no local pointer")
                return {"status": "no-remote-pointer", "build_id": None, "files": 0}
            raise
        validate_pointer(pointer)
        build_id = pointer["build_id"]
        manifest = validate_manifest(
            read_json_object(
                s3, bucket, f"{GENERATION_PREFIX}{build_id}/manifest.json", MAX_MANIFEST_BYTES
            ),
            pointer,
        )
        target = generations / build_id
        if target.exists() or target.is_symlink():
            verify_existing_generation(target, manifest)
            installed = False
        else:
            stage = _stage_generation(s3, bucket, manifest, destination)
            installed = _install(stage, target, manifest)
        activated = _read_local_pointer(destination) != docs_index.pointer_for(manifest)
        if activated:
            docs_index.activate(destination, manifest)
        status = "installed" if installed else ("activated" if activated else "current")
        log.info("Docs generation %s: %s (%d files)", build_id, status, len(manifest["files"]))
        return {"status": status, "build_id": build_id, "files": len(manifest["files"])}


def main(argv=None):
    parser = argparse.ArgumentParser(description="Sync the docs MCP generation from S3.")
    parser.add_argument("--bucket", default=os.environ.get("DOCS_BUCKET"), help="S3 bucket name")
    parser.add_argument(
        "--root", default=os.environ.get("DOCS_DB_PATH"), help="Local docs_db destination"
    )
    parser.add_argument(
        "--allow-missing-pointer",
        action="store_true",
        help="Succeed without a remote pointer only when no local current.json exists yet",
    )
    parser.add_argument("--lock-timeout", type=float, default=300.0)
    args = parser.parse_args(argv)
    if not args.bucket or not args.root:
        parser.error("--bucket and --root (or DOCS_BUCKET / DOCS_DB_PATH) are required")
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    import boto3

    try:
        result = sync(
            boto3.client("s3"),
            args.bucket,
            args.root,
            allow_missing_pointer=args.allow_missing_pointer,
            lock_timeout=args.lock_timeout,
        )
    except Exception as error:
        # SDK and filesystem exceptions can carry endpoint, credential, or path details.
        log.error("Docs sync failed: %s", type(error).__name__)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
