"""Bounded extraction of labelled entities from PDF documents."""

from __future__ import annotations

import io
import os
import re
import stat
from pathlib import Path
from typing import Any, Iterable

from .schema import Entity

DEFAULT_MAX_BYTES = 25 * 1024 * 1024
DEFAULT_MAX_PAGES = 100
DEFAULT_MAX_VALUE_LENGTH = 120

LINE = re.compile(r"^\s*([A-Za-z][A-Za-z0-9 .'/#&()-]{1,40}?)\s*[:\u2013-]\s+(.+?)\s*$")


class PdfError(RuntimeError):
    """A stable, serializable PDF validation or extraction failure."""

    def __init__(self, code: str, message: str, **details: Any) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details

    def to_dict(self) -> dict[str, Any]:
        return {
            "code": self.code,
            "message": self.message,
            "details": self.details,
        }


def resolve_pdf_path(
    pdf_path: str | Path,
    *,
    allowed_roots: Iterable[str | Path] | None = None,
    base_dir: str | Path | None = None,
    max_bytes: int = DEFAULT_MAX_BYTES,
) -> Path:
    """Resolve a PDF below an allowed root and enforce basic resource bounds.

    With no explicit roots, access is confined to ``base_dir`` or the current
    working directory. Symlinks are resolved before the containment check.
    """

    if max_bytes <= 0:
        raise PdfError("invalid_limit", "max_bytes must be positive", max_bytes=max_bytes)

    base = Path(base_dir or Path.cwd()).expanduser().resolve()
    roots = tuple(Path(root).expanduser().resolve() for root in (allowed_roots or (base,)))
    if not roots:
        raise PdfError("no_allowed_roots", "At least one allowed PDF root is required")

    candidate = Path(pdf_path).expanduser()
    if not candidate.is_absolute():
        candidate = base / candidate
    try:
        candidate = candidate.resolve(strict=True)
    except (OSError, RuntimeError) as exc:
        raise PdfError(
            "pdf_not_found", "PDF path could not be resolved", path=str(pdf_path)
        ) from exc

    if not any(candidate == root or candidate.is_relative_to(root) for root in roots):
        raise PdfError(
            "pdf_path_not_allowed",
            "PDF path is outside the configured allowed roots",
            path=str(candidate),
            allowed_roots=[str(root) for root in roots],
        )
    if candidate.suffix.lower() != ".pdf":
        raise PdfError("invalid_pdf_extension", "Only .pdf files are accepted", path=str(candidate))
    if not candidate.is_file():
        raise PdfError("pdf_not_file", "PDF path is not a regular file", path=str(candidate))

    try:
        size = candidate.stat().st_size
    except OSError as exc:
        raise PdfError(
            "pdf_stat_failed", "Could not inspect PDF size", path=str(candidate)
        ) from exc
    if size > max_bytes:
        raise PdfError(
            "pdf_too_large",
            "PDF exceeds the configured byte limit",
            path=str(candidate),
            size_bytes=size,
            max_bytes=max_bytes,
        )
    return candidate


def extract_entities(
    pdf_path: str | Path,
    *,
    allowed_roots: Iterable[str | Path] | None = None,
    base_dir: str | Path | None = None,
    max_bytes: int = DEFAULT_MAX_BYTES,
    max_pages: int = DEFAULT_MAX_PAGES,
    max_value_length: int = DEFAULT_MAX_VALUE_LENGTH,
) -> list[Entity]:
    """Extract ``Label: value`` pairs without allowing unbounded file access."""

    if max_pages <= 0 or max_value_length <= 0:
        raise PdfError(
            "invalid_limit",
            "max_pages and max_value_length must be positive",
            max_pages=max_pages,
            max_value_length=max_value_length,
        )
    path = resolve_pdf_path(
        pdf_path,
        allowed_roots=allowed_roots,
        base_dir=base_dir,
        max_bytes=max_bytes,
    )

    try:
        import pdfplumber
    except ImportError as exc:  # pragma: no cover - depends on optional runtime extra
        raise PdfError(
            "pdf_dependency_missing",
            "PDF extraction requires the optional 'pdfplumber' dependency",
        ) from exc

    data = _read_validated_pdf(path, max_bytes=max_bytes)
    entities: list[Entity] = []
    seen: set[tuple[str, str]] = set()
    try:
        with pdfplumber.open(io.BytesIO(data)) as pdf:
            if len(pdf.pages) > max_pages:
                raise PdfError(
                    "pdf_too_many_pages",
                    "PDF exceeds the configured page limit",
                    path=str(path),
                    pages=len(pdf.pages),
                    max_pages=max_pages,
                )
            for page in pdf.pages:
                for line in (page.extract_text() or "").splitlines():
                    match = LINE.match(line)
                    if not match:
                        continue
                    label, value = match.group(1).strip(), match.group(2).strip()
                    pair = (label, value)
                    if len(value) > max_value_length or pair in seen:
                        continue
                    seen.add(pair)
                    entities.append(Entity(label, value))
    except PdfError:
        raise
    except Exception as exc:  # pdfplumber surfaces parser-specific exceptions
        raise PdfError(
            "pdf_extract_failed", "Could not extract text from PDF", path=str(path)
        ) from exc
    return derive_entities(entities)


def _read_validated_pdf(path: Path, *, max_bytes: int) -> bytes:
    """Read a regular non-symlink file into bounded bytes before parsing."""

    try:
        expected = path.stat(follow_symlinks=False)
    except OSError as exc:
        raise PdfError("pdf_stat_failed", "Could not inspect PDF", path=str(path)) from exc
    if not stat.S_ISREG(expected.st_mode):
        raise PdfError("pdf_not_file", "PDF path is not a regular file", path=str(path))

    flags = os.O_RDONLY
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        raise PdfError("pdf_open_failed", "Could not safely open PDF", path=str(path)) from exc
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise PdfError("pdf_not_file", "PDF path is not a regular file", path=str(path))
        if (metadata.st_dev, metadata.st_ino) != (expected.st_dev, expected.st_ino):
            raise PdfError(
                "pdf_changed_before_open",
                "PDF changed while it was being opened",
                path=str(path),
            )
        if metadata.st_size > max_bytes:
            raise PdfError(
                "pdf_too_large",
                "PDF exceeds the configured byte limit",
                path=str(path),
                size_bytes=metadata.st_size,
                max_bytes=max_bytes,
            )
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            data = stream.read(max_bytes + 1)
        if len(data) > max_bytes:
            raise PdfError(
                "pdf_too_large",
                "PDF exceeds the configured byte limit",
                path=str(path),
                size_bytes=len(data),
                max_bytes=max_bytes,
            )
        return data
    finally:
        os.close(descriptor)


def derive_entities(entities: list[Entity]) -> list[Entity]:
    """Add conservative first/last/full-name variants."""

    labels = {entity.label.lower(): entity for entity in entities}
    output = list(entities)
    name = next(
        (
            entity
            for label, entity in labels.items()
            if label in {"name", "full name", "patient", "applicant", "patient name", "legal name"}
        ),
        None,
    )
    if name and " " in name.value.strip() and "first name" not in labels:
        first, _, last = name.value.strip().partition(" ")
        output.append(Entity("First name", first))
        output.append(Entity("Last name", last.strip()))
    if "first name" in labels and "last name" in labels and not name:
        output.append(
            Entity("Full name", f"{labels['first name'].value} {labels['last name'].value}")
        )
    return output


# Compatibility with the initial research prototype.
derive = derive_entities
