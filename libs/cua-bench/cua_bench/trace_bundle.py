"""Experimental fork-only trace bundles; RFC #5685 is not yet accepted."""

from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import stat
import tempfile
import uuid
import weakref
from datetime import datetime, timezone
from pathlib import Path

FORMAT = "cua-bench-trace"
VERSION = 1
MAX_EVENT = 1024 * 1024
MAX_ATTACHMENT = 100 * 1024 * 1024  # default import/download limit
HARD_MAX_ATTACHMENT = 4 * 1024**3
MAX_TEXT_PREVIEW = 256 * 1024
MAX_IMAGE_PREVIEW = 8 * 1024**2


def utc_now():
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds")


def validate_context(context):
    if context is not None and not isinstance(context, dict):
        raise ValueError("trace context must be an object")
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
    correlations = context.get("correlations", [])
    if not isinstance(correlations, list) or len(correlations) > 32:
        raise ValueError("correlations must be a list of at most 32 identifiers")
    for correlation in correlations:
        if (
            not isinstance(correlation, dict)
            or set(correlation) != {"namespace", "kind", "id", "provenance"}
            or any(
                not isinstance(value, str) or not 1 <= len(value) <= 256
                for value in correlation.values()
            )
        ):
            raise ValueError(
                "correlation requires namespace, kind, id and provenance strings (1-256 characters)"
            )
    json.dumps(context, allow_nan=False)
    return context


class TraceBundleWriter:
    """Single-owner append journal. Existing directories are never overwritten.

    Use as a context manager. A successful append is flushed for readers; close
    fsyncs the journal. An unclosed bundle remains incomplete, even after a crash.
    """

    def __init__(self, directory, *, max_attachment=MAX_ATTACHMENT, quota=1024**3):
        if (
            type(max_attachment) is not int
            or type(quota) is not int
            or not 0 < max_attachment <= HARD_MAX_ATTACHMENT
            or quota <= 0
        ):
            raise ValueError("invalid attachment limit or bundle quota")
        self.path = Path(directory).expanduser().resolve()
        self.path.mkdir(parents=True, exist_ok=False)
        (self.path / "artifacts").mkdir()
        self.generation = str(uuid.uuid4())
        self.created_at = utc_now()
        self.sequence = 0
        self.closed = False
        self.failed = False
        self.max_attachment = max_attachment
        self.quota = quota
        self.used = 0
        self._registry_dir = tempfile.TemporaryDirectory(prefix="cua-trace-writer-")
        self.attachments = None
        self.journal = None
        try:
            self.attachments = sqlite3.connect(Path(self._registry_dir.name) / "registry.sqlite")
            self.attachments.execute("PRAGMA cache_size=-2048")
            self.attachments.execute("CREATE TABLE attachments (id TEXT PRIMARY KEY, body TEXT)")
            self.journal = (self.path / "events.jsonl").open("xb")
            self._manifest("recording")
        except BaseException:
            if self.journal is not None:
                self.journal.close()
            if self.attachments is not None:
                self.attachments.close()
            self._registry_dir.cleanup()
            raise

    def _manifest(self, state):
        manifest = {
            "format": FORMAT,
            "version": VERSION,
            "generation": self.generation,
            "state": state,
            "created_at": self.created_at,
            "clock": {"recorded_at": "UTC wall clock", "aligned_at": "collector-supplied UTC"},
            "limits": {
                "event_bytes": MAX_EVENT,
                "attachment_bytes": self.max_attachment,
                "bundle_bytes": self.quota,
            },
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
        self.attachments.execute(
            "INSERT INTO attachments VALUES (?, ?)", (artifact_id, json.dumps(descriptor))
        )
        self.attachments.commit()
        return dict(descriptor)

    def append(self, row, *, context=None, attachments=None):
        self._writable()
        context = validate_context(context)
        refs = list(attachments or [])
        for ref in refs:
            stored = (
                self.attachments.execute(
                    "SELECT body FROM attachments WHERE id=?", (ref.get("id"),)
                ).fetchone()
                if isinstance(ref, dict)
                else None
            )
            if not stored or json.loads(stored[0]) != ref:
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
            self.attachments.close()
            self._registry_dir.cleanup()

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
        self.count = 0
        self.identity = None
        # A disposable disk index: never mutate the recording being inspected.
        self._cache = tempfile.TemporaryDirectory(prefix="cua-trace-index-")
        self.db = sqlite3.connect(Path(self._cache.name) / "index.sqlite", check_same_thread=False)
        self._finalizer = weakref.finalize(self, self._cleanup, self.db, self._cache)
        self.db.execute("PRAGMA cache_size=-2048")
        self.db.execute("PRAGMA temp_store=FILE")
        self.db.executescript(
            """
            CREATE TABLE events (sequence INTEGER PRIMARY KEY, aligned TEXT,
                unaligned INTEGER, domain TEXT, has_images INTEGER, body TEXT);
            CREATE INDEX observation_order ON events(unaligned, aligned, domain, sequence);
            CREATE TABLE attachments (id TEXT PRIMARY KEY, body TEXT);
            CREATE TABLE correlations (sequence INTEGER, identity TEXT, PRIMARY KEY(sequence, identity));
            CREATE INDEX correlation_identity ON correlations(identity, sequence);
        """
        )
        try:
            self.refresh()
        except BaseException:
            self.close()
            raise

    @staticmethod
    def _cleanup(db, cache):
        db.close()
        cache.cleanup()

    def close(self):
        self._finalizer()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def refresh(self):
        manifest = json.loads((self.path / "trace.json").read_text(encoding="utf-8"))
        if manifest.get("format") != FORMAT or manifest.get("version") != VERSION:
            raise ValueError("unsupported trace bundle version")
        if (
            manifest.get("state") not in ("recording", "completed", "failed")
            or not isinstance(manifest.get("generation"), str)
            or not manifest["generation"]
        ):
            raise ValueError("invalid trace manifest")
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
                if event.get("sequence") != self.count:
                    raise ValueError("trace sequence is corrupt")
                if not isinstance(event.get("event_name"), str) or not isinstance(
                    event.get("recorded_at"), str
                ):
                    raise ValueError("invalid trace event metadata")
                if not isinstance(event.get("data"), dict):
                    raise ValueError("trace event data must be an object")
                event["context"] = validate_context(event.get("context"))
                if not isinstance(event.get("images"), list) or not isinstance(
                    event.get("attachments"), list
                ):
                    raise ValueError("invalid trace attachment list")
                for ref in event["images"] + event["attachments"]:
                    if not isinstance(ref, dict) or not isinstance(ref.get("id"), str):
                        raise ValueError("invalid attachment identity")
                    if (
                        not isinstance(ref.get("size"), int)
                        or not 0 <= ref["size"] <= HARD_MAX_ATTACHMENT
                        or not isinstance(ref.get("name"), str)
                        or not isinstance(ref.get("media_type"), str)
                        or not isinstance(ref.get("sha256"), str)
                        or len(ref["sha256"]) != 64
                        or any(c not in "0123456789abcdef" for c in ref["sha256"])
                    ):
                        raise ValueError("invalid attachment descriptor")
                    artifact_id = ref["id"]
                    if (
                        len(artifact_id) != 32
                        or any(c not in "0123456789abcdef" for c in artifact_id)
                        or ref.get("generation") != self.generation
                    ):
                        raise ValueError("invalid attachment identity")
                    existing = self.db.execute(
                        "SELECT body FROM attachments WHERE id=?", (artifact_id,)
                    ).fetchone()
                    if existing and json.loads(existing[0]) != ref:
                        raise ValueError("conflicting attachment descriptor")
                    self.db.execute(
                        "INSERT OR IGNORE INTO attachments VALUES (?, ?)",
                        (artifact_id, json.dumps(ref)),
                    )
                context = event["context"]
                aligned = context.get("aligned_at")
                self.db.execute(
                    "INSERT INTO events VALUES (?, ?, ?, ?, ?, ?)",
                    (
                        self.count,
                        aligned or "",
                        int(aligned is None),
                        str(context.get("clock_domain", "unknown")) if aligned is None else "",
                        int(bool(event.get("images"))),
                        json.dumps(event),
                    ),
                )
                for correlation in context.get("correlations", []):
                    identity = json.dumps(
                        [
                            event["trajectory_id"],
                            *[
                                correlation[key]
                                for key in ("namespace", "kind", "id", "provenance")
                            ],
                        ]
                    )
                    self.db.execute(
                        "INSERT OR IGNORE INTO correlations VALUES (?, ?)", (self.count, identity)
                    )
                self.count += 1
                self.offset = handle.tell()
        self.db.commit()
        if manifest["state"] == "completed":
            if manifest["final_sequence"] != self.count - 1 or info.st_size != self.offset:
                raise ValueError("completed trace is incomplete")
        return manifest

    def batch(self, after=-1, *, generation=None):
        manifest = self.refresh()
        if generation is not None and generation != self.generation:
            raise ValueError("trace generation changed; reopen the viewer")
        if after < -1 or after >= self.count:
            raise ValueError("invalid trace cursor")
        records = []
        size = 0
        for (body,) in self.db.execute(
            "SELECT body FROM events WHERE sequence>? ORDER BY sequence LIMIT 500", (after,)
        ):
            event = json.loads(body)
            size += len(json.dumps(event).encode())
            if records and size > 2 * 1024 * 1024:
                break
            records.append(event)
        cursor = records[-1]["sequence"] if records else after
        return {
            "generation": self.generation,
            "events": records,
            "cursor": cursor,
            "more": cursor < self.count - 1,
            "state": manifest["state"],
            "final_sequence": manifest.get("final_sequence"),
        }

    def status(self, *, cutoff=None):
        manifest = self.refresh()
        last = self.count - 1 if cutoff is None else min(cutoff, self.count - 1)
        return {
            "generation": self.generation,
            "cursor": last,
            "total": last + 1,
            "state": manifest["state"] if cutoff is None else "snapshot",
            "final_sequence": manifest.get("final_sequence"),
        }

    def page(self, *, order="arrival", page=0, through=None, selected=None):
        """Bounded summaries at a stable journal watermark, even during late insertion."""
        if order not in ("arrival", "observed") or page < 0:
            raise ValueError("invalid trace page")
        through = self.count - 1 if through is None else through
        if through < -1 or through >= self.count:
            raise ValueError("invalid trace cursor")
        ordering = "sequence" if order == "arrival" else "unaligned, aligned, domain, sequence"
        # Ranking lives on disk; event bodies are only loaded for one page.
        if selected is not None:
            row = self.db.execute(
                f"SELECT position FROM (SELECT sequence, row_number() OVER (ORDER BY {ordering}) - 1 AS position FROM events WHERE sequence<=?) WHERE sequence=?",
                (through, selected),
            ).fetchone()
            if row:
                page = row[0] // 100
        page = min(page, max(0, through // 100))
        rows = self.db.execute(
            f"SELECT body FROM events WHERE sequence<=? ORDER BY {ordering} LIMIT 100 OFFSET ?",
            (through, page * 100),
        )
        summaries = []
        for (body,) in rows:
            event = json.loads(body)
            summaries.append(
                {
                    "sequence": event["sequence"],
                    "event_name": event["event_name"][:512],
                    "recorded_at": event["recorded_at"][:512],
                    "context": {
                        key: str(event["context"][key])[:512]
                        for key in ("aligned_at", "clock_domain")
                        if key in event["context"]
                    },
                }
            )
        return {
            "events": summaries,
            "page": page,
            "total": through + 1,
            "cursor": through,
            "generation": self.generation,
        }

    def event(self, sequence):
        row = self.db.execute("SELECT body FROM events WHERE sequence=?", (sequence,)).fetchone()
        if row is None:
            raise ValueError("unknown event")
        from .trace_snapshot import snapshot_boxes

        event = json.loads(row[0])
        event["snapshot_boxes"] = snapshot_boxes(event["data"])
        return event

    def related(self, sequence, *, order="arrival", through=None, page=0):
        """Exact supplied identifiers in one trajectory; never timestamp inference."""
        through = self.count - 1 if through is None else through
        if order not in ("arrival", "observed") or page < 0:
            raise ValueError("invalid related evidence page")
        if not 0 <= sequence <= through < self.count:
            raise ValueError("unknown event")
        ordering = (
            "e.sequence" if order == "arrival" else "e.unaligned, e.aligned, e.domain, e.sequence"
        )
        match = """FROM events e WHERE e.sequence<=? AND e.sequence!=? AND e.sequence IN
            (SELECT c.sequence FROM correlations c JOIN correlations selected
             ON c.identity=selected.identity WHERE selected.sequence=?)"""
        args = (through, sequence, sequence)
        total = self.db.execute("SELECT count(*) " + match, args).fetchone()[0]
        page = min(page, max(0, (total - 1) // 100))
        records = self.db.execute(
            "SELECT e.body " + match + f" ORDER BY {ordering} LIMIT 100 OFFSET ?",
            (*args, page * 100),
        )
        events = []
        for (body,) in records:
            event = json.loads(body)
            events.append({"sequence": event["sequence"], "event_name": event["event_name"][:512]})
        return {
            "events": events,
            "total": total,
            "page": page,
            "cursor": through,
            "generation": self.generation,
        }

    def image_neighbor(self, sequence, *, direction, order="arrival", through=None):
        if direction not in (-1, 1) or order not in ("arrival", "observed"):
            raise ValueError("invalid image navigation")
        through = self.count - 1 if through is None else through
        ordering = "sequence" if order == "arrival" else "unaligned, aligned, domain, sequence"
        # Include the selected event even when it has no image.
        rows = self.db.execute(
            f"WITH ranked AS (SELECT sequence, row_number() OVER (ORDER BY {ordering}) AS pos FROM events WHERE sequence<=? AND (has_images=1 OR sequence=?)) SELECT sequence FROM ranked WHERE pos=(SELECT pos+? FROM ranked WHERE sequence=?)",
            (through, sequence, direction, sequence),
        ).fetchone()
        return self.event(rows[0]) if rows else None

    def attachment_ref(self, artifact_id):
        row = self.db.execute("SELECT body FROM attachments WHERE id=?", (artifact_id,)).fetchone()
        if row is None:
            raise ValueError("unknown attachment")
        return json.loads(row[0])

    def attachment(self, artifact_id, *, max_size=MAX_ATTACHMENT):
        """Return validated bytes in a temporary file; callers must close it."""
        ref = self.attachment_ref(artifact_id)
        if ref["size"] > max_size:
            raise ValueError("attachment exceeds viewer download limit")
        return self.attachment_snapshot(ref)

    def attachment_snapshot(self, ref):
        """Copy an indexed descriptor into verified temporary storage, without using SQLite."""
        artifact_id = ref["id"]
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
