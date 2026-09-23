from __future__ import annotations

import os
import sys
from types import SimpleNamespace

import pytest

from cua_s1.pdf import PdfError, derive_entities, extract_entities, resolve_pdf_path
from cua_s1.schema import Entity


def test_pdf_path_is_confined_to_allowed_roots(tmp_path):
    allowed = tmp_path / "allowed"
    allowed.mkdir()
    inside = allowed / "record.pdf"
    inside.write_bytes(b"%PDF-1.4\n")
    outside = tmp_path / "outside.pdf"
    outside.write_bytes(b"%PDF-1.4\n")

    assert resolve_pdf_path(inside, allowed_roots=[allowed]) == inside.resolve()
    with pytest.raises(PdfError) as caught:
        resolve_pdf_path(outside, allowed_roots=[allowed])
    assert caught.value.code == "pdf_path_not_allowed"


def test_pdf_path_rejects_symlink_escape_and_oversized_file(tmp_path):
    allowed = tmp_path / "allowed"
    allowed.mkdir()
    outside = tmp_path / "outside.pdf"
    outside.write_bytes(b"%PDF-1.4\n")
    link = allowed / "linked.pdf"
    try:
        link.symlink_to(outside)
    except OSError:
        pytest.skip("symlinks are not available")

    with pytest.raises(PdfError) as escaped:
        resolve_pdf_path(link, allowed_roots=[allowed])
    assert escaped.value.code == "pdf_path_not_allowed"

    local = allowed / "large.pdf"
    local.write_bytes(b"12345")
    with pytest.raises(PdfError) as oversized:
        resolve_pdf_path(local, allowed_roots=[allowed], max_bytes=4)
    assert oversized.value.code == "pdf_too_large"
    assert oversized.value.details["size_bytes"] == 5


def test_extract_entities_enforces_page_bound_before_text_extraction(monkeypatch, tmp_path):
    path = tmp_path / "record.pdf"
    path.write_bytes(b"%PDF-1.4\n")

    class Page:
        def extract_text(self):
            raise AssertionError("text extraction must not begin after the page limit is exceeded")

    class Document:
        pages = [Page(), Page()]

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return None

    monkeypatch.setitem(sys.modules, "pdfplumber", SimpleNamespace(open=lambda _stream: Document()))

    with pytest.raises(PdfError) as caught:
        extract_entities(path, allowed_roots=[tmp_path], max_pages=1)
    assert caught.value.code == "pdf_too_many_pages"
    assert caught.value.details == {"path": str(path), "pages": 2, "max_pages": 1}


def test_extract_entities_refuses_a_symlink_swap_before_open(monkeypatch, tmp_path):
    path = tmp_path / "record.pdf"
    path.write_bytes(b"%PDF-1.4\n")
    outside = tmp_path.parent / "outside.pdf"
    outside.write_bytes(b"%PDF-1.4\nsecret")
    real_open = os.open
    swapped = False

    def swap_then_open(candidate, flags):
        nonlocal swapped
        if not swapped and candidate == path.resolve():
            swapped = True
            path.unlink()
            path.symlink_to(outside)
        return real_open(candidate, flags)

    monkeypatch.setattr(os, "open", swap_then_open)

    with pytest.raises(PdfError) as caught:
        extract_entities(path, allowed_roots=[tmp_path])
    assert caught.value.code == "pdf_open_failed"


def test_derive_entities_adds_only_missing_name_variants():
    derived = derive_entities([Entity("Full name", "Avery Chen")])
    assert derived == [
        Entity("Full name", "Avery Chen"),
        Entity("First name", "Avery"),
        Entity("Last name", "Chen"),
    ]

    combined = derive_entities([Entity("First name", "Avery"), Entity("Last name", "Chen")])
    assert combined[-1] == Entity("Full name", "Avery Chen")
