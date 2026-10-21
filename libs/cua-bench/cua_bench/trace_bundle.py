"""Experimental fork-only trace bundles; RFC #5685 is not yet accepted."""

from __future__ import annotations

import hashlib
import json
import os
import stat
import tempfile
import uuid
from datetime import datetime, timezone
from pathlib import Path

FORMAT = "cua-bench-trace"
VERSION = 1
MAX_EVENT = 1024 * 1024
MAX_ATTACHMENT = 100 * 1024 * 1024


def utc_now():
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds")


def validate_context(context):
    context = dict(context or {})
    aligned = context.get("aligned_at")
    if aligned is not None:
        if not isinstance(aligned, str):
            raise ValueError("aligned_at must be an ISO timestamp with timezone")
        try:
            parsed = datetime.fromisoformat(aligned.replace("Z", "+00:00"))
        except ValueError as error:
            raise ValueError("aligned_at must be an ISO timestamp with timezone") from error
        if parsed.tzinfo is None:
            raise ValueError("aligned_at must include a timezone")
        if not context.get("clock_domain"):
            raise ValueError("aligned_at requires clock_domain provenance")
        context["aligned_at"] = parsed.astimezone(timezone.utc).isoformat(timespec="milliseconds")
    json.dumps(context, allow_nan=False)
    return context


class TraceBundleWriter:
    """Single-owner append journal. Existing directories are never overwritten.

    Use as a context manager. A successful append is flushed for readers; close
    fsyncs the journal. An unclosed bundle remains incomplete, even after a crash.
    """

    def __init__(self, directory, *, max_attachment=MAX_ATTACHMENT, quota=1024**3):
        self.path = Path(directory).expanduser().resolve()
        self.path.mkdir(parents=True, exist_ok=False)
        (self.path / "artifacts").mkdir()
        self.generation = str(uuid.uuid4())
        self.sequence = 0
        self.closed = False
        self.failed = False
        self.max_attachment = max_attachment
        self.quota = quota
        self.used = 0
        self.attachments = {}
        self.journal = (self.path / "events.jsonl").open("xb")
        self._manifest("recording")

    def _manifest(self, state):
        manifest = {
            "format": FORMAT,
            "version": VERSION,
            "generation": self.generation,
            "state": state,
            "final_sequence": self.sequence - 1 if state == "completed" else None,
        }
        temporary = self.path / "trace.json.tmp"
        with temporary.open("w", encoding="utf-8") as handle:
            json.dump(manifest, handle)
            handle.flush()
            os.fsync(handle.fileno())
        temporary.replace(self.path / "trace.json")

    def _writable(self):
        if self.closed or self.failed:
            raise RuntimeError("trace writer is closed or failed")

    def add_attachment(self, path, *, name=None, media_type="application/octet-stream"):
        """Copy a regular file. The returned descriptor belongs to this writer."""
        self._writable()
        source = Path(path)
        if source.is_symlink() or not source.is_file():
            raise ValueError("attachment must be a regular non-symlink file")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_BINARY", 0)
        with os.fdopen(os.open(source, flags), "rb") as handle:
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                raise ValueError("attachment must be a regular file")
            return self._import(handle, name or source.name, media_type)

    def add_image(self, image):
        from io import BytesIO

        from PIL import Image

        self._writable()
        if isinstance(image, (bytes, bytearray)):
            image = Image.open(BytesIO(image))
        buffer = BytesIO()
        image.save(buffer, format="PNG")
        buffer.seek(0)
        return self._import(buffer, "screenshot.png", "image/png")

    def _import(self, source, name, media_type):
        if not isinstance(name, str) or not isinstance(media_type, str):
            raise ValueError("attachment name and media type must be strings")
        artifact_id = uuid.uuid4().hex
        target = self.path / "artifacts" / artifact_id
        temporary = target.with_suffix(".tmp")
        digest = hashlib.sha256()
        size = 0
        try:
            with temporary.open("xb") as dest:
                while chunk := source.read(64 * 1024):
                    size += len(chunk)
                    if size > self.max_attachment or self.used + size > self.quota:
                        raise ValueError("attachment size or bundle quota exceeded")
                    dest.write(chunk)
                    digest.update(chunk)
                dest.flush()
                os.fsync(dest.fileno())
            temporary.replace(target)
        finally:
            temporary.unlink(missing_ok=True)
        descriptor = {
            "id": artifact_id,
            "generation": self.generation,
            "name": name,
            "media_type": media_type,
            "size": size,
            "sha256": digest.hexdigest(),
        }
        self.used += size
        self.attachments[artifact_id] = descriptor
        return dict(descriptor)

    def append(self, row, *, context=None, attachments=None):
        self._writable()
        context = validate_context(context)
        refs = list(attachments or [])
        for ref in refs:
            if not isinstance(ref, dict) or self.attachments.get(ref.get("id")) != ref:
                raise ValueError("attachment does not belong to this writer")
        event = {
            "sequence": self.sequence,
            "trajectory_id": row["trajectory_id"],
            "event_name": row["event_name"],
            "recorded_at": row["timestamp"],
            "context": context,
            "data": json.loads(row["data_json"]),
            "images": [],
            "attachments": refs,
        }
        # Reject oversized metadata before importing any images.
        if len(json.dumps(event, allow_nan=False).encode()) > MAX_EVENT:
            raise ValueError("event exceeds size limit")
        event["images"] = [self.add_image(image) for image in row.get("data_images", [])]
        line = (json.dumps(event, ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")
        if len(line) > MAX_EVENT or self.used + len(line) > self.quota:
            raise ValueError("event size or bundle quota exceeded")
        try:
            self.journal.write(line)
            self.journal.flush()
        except OSError:
            self.failed = True
            raise
        self.used += len(line)
        self.sequence += 1

    def close(self, *, failed=False):
        if self.closed:
            return
        try:
            self.journal.flush()
            os.fsync(self.journal.fileno())
            self._manifest("failed" if failed or self.failed else "completed")
        finally:
            self.closed = True
            self.journal.close()

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        self.close(failed=exc_type is not None)


class TraceBundleReader:
    """Incremental reader. A partial trailing line is never returned as an event."""

    def __init__(self, directory):
        self.path = Path(directory).resolve()
        self.generation = None
        self.offset = 0
        self.events = []
        self.attachments = {}
        self.identity = None
        self.refresh()

    def refresh(self):
        manifest = json.loads((self.path / "trace.json").read_text(encoding="utf-8"))
        if manifest.get("format") != FORMAT or manifest.get("version") != VERSION:
            raise ValueError("unsupported trace bundle version")
        if self.generation and manifest["generation"] != self.generation:
            raise ValueError("trace generation changed; reopen the viewer")
        self.generation = manifest["generation"]
        with (self.path / "events.jsonl").open("rb") as handle:
            info = os.fstat(handle.fileno())
            identity = (info.st_dev, info.st_ino)
            if (self.identity and identity != self.identity) or info.st_size < self.offset:
                raise ValueError("trace journal replaced or truncated; reopen the viewer")
            self.identity = identity
            handle.seek(self.offset)
            while True:
                line = handle.readline(MAX_EVENT + 1)
                if len(line) > MAX_EVENT:
                    raise ValueError("trace event exceeds size limit")
                if not line or not line.endswith(b"\n"):
                    break
                event = json.loads(line)
                if event.get("sequence") != len(self.events):
                    raise ValueError("trace sequence is corrupt")
                if not isinstance(event.get("data"), dict):
                    raise ValueError("trace event data must be an object")
                event["context"] = validate_context(event.get("context"))
                for ref in event.get("images", []) + event.get("attachments", []):
                    artifact_id = ref["id"]
                    if (
                        len(artifact_id) != 32
                        or any(c not in "0123456789abcdef" for c in artifact_id)
                        or ref.get("generation") != self.generation
                    ):
                        raise ValueError("invalid attachment identity")
                    if artifact_id in self.attachments and self.attachments[artifact_id] != ref:
                        raise ValueError("conflicting attachment descriptor")
                    self.attachments[artifact_id] = ref
                self.events.append(event)
                self.offset = handle.tell()
        if manifest["state"] == "completed":
            if manifest["final_sequence"] != len(self.events) - 1 or info.st_size != self.offset:
                raise ValueError("completed trace is incomplete")
        return manifest

    def batch(self, after=-1, *, generation=None):
        manifest = self.refresh()
        if generation is not None and generation != self.generation:
            raise ValueError("trace generation changed; reopen the viewer")
        if after < -1 or after >= len(self.events):
            raise ValueError("invalid trace cursor")
        records = []
        size = 0
        for event in self.events[after + 1 : after + 501]:
            size += len(json.dumps(event).encode())
            if records and size > 2 * 1024 * 1024:
                break
            records.append(event)
        cursor = records[-1]["sequence"] if records else after
        return {
            "generation": self.generation,
            "events": records,
            "cursor": cursor,
            "more": cursor < len(self.events) - 1,
            "state": manifest["state"],
            "final_sequence": manifest.get("final_sequence"),
        }

    def attachment(self, artifact_id):
        """Return validated bytes in a temporary file; callers must close it."""
        ref = self.attachments.get(artifact_id)
        if ref is None:
            raise ValueError("unknown attachment")
        root = self.path / "artifacts"
        source = root / artifact_id
        if root.is_symlink() or source.is_symlink() or not source.is_file():
            raise ValueError("attachment unavailable")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_BINARY", 0)
        result = tempfile.TemporaryFile()
        try:
            with os.fdopen(os.open(source, flags), "rb") as handle:
                info = os.fstat(handle.fileno())
                if not stat.S_ISREG(info.st_mode) or info.st_size != ref["size"]:
                    raise ValueError("attachment size mismatch")
                digest = hashlib.sha256()
                size = 0
                while chunk := handle.read(64 * 1024):
                    size += len(chunk)
                    if size > ref["size"]:
                        raise ValueError("attachment size mismatch")
                    digest.update(chunk)
                    result.write(chunk)
                if size != ref["size"] or digest.hexdigest() != ref["sha256"]:
                    raise ValueError("attachment digest mismatch")
            result.seek(0)
            return result, ref
        except BaseException:
            result.close()
            raise
