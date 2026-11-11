#!/usr/bin/env python3
"""Build the deterministic CDB-S02 ODS fixture using only the standard library."""

from __future__ import annotations

import argparse
import hashlib
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
BUNDLE = HERE.parent
OUTPUT = BUNDLE / "fixture" / "planning" / "lighthouse-replan.ods"
STAMP = (2026, 1, 1, 0, 0, 0)

MIMETYPE = b"application/vnd.oasis.opendocument.spreadsheet"
MANIFEST = b'''<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">
 <manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>
</manifest:manifest>
'''
STYLES = b'''<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3"><office:styles/></office:document-styles>
'''
META = b'''<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" office:version="1.3"><office:meta><meta:generator>Cua Driver Bench</meta:generator></office:meta></office:document-meta>
'''

SCHEDULE = [
    ["Session", "Status", "Start", "End", "Room", "Constraint / Blocker"],
    ["LHP-ASTER", "tentative", "09:00", "09:30", "Juniper", "Mail revision 3"],
    ["LHP-BOREAL", "tentative", "10:00", "11:00", "Sable", "Mail revision 3"],
    ["LHP-CINDER", "tentative", "11:30", "12:00", "Juniper", "Mail revision 3"],
    ["LHP-DUNE", "tentative", "14:30", "15:00", "Sable", "Mail revision 3"],
]
CONTROLS = [
    ["Task", "LHP-2026-10-14"],
    ["Timezone", "America/Chicago"],
    ["Mail revision", "3"],
    ["Chat revision", "4"],
    ["Status vocabulary", "scheduled | cancelled | blocked:<conflict-id>"],
    ["Unscheduled fields", "Leave Start, End, and Room blank"],
    ["Evidence notes", "Cite the authoritative revision or conflicting calendar event"],
]


def esc(value: str) -> str:
    return value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def table(name: str, rows: list[list[str]]) -> str:
    body = []
    for row in rows:
        cells = "".join(
            f'<table:table-cell office:value-type="string"><text:p>{esc(value)}</text:p></table:table-cell>'
            for value in row
        )
        body.append(f"<table:table-row>{cells}</table:table-row>")
    return f'<table:table table:name="{name}">{"".join(body)}</table:table>'


def content() -> bytes:
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" '
        'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
        'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3">'
        '<office:body><office:spreadsheet>' + table("Schedule", SCHEDULE) + table("Controls", CONTROLS) +
        '</office:spreadsheet></office:body></office:document-content>\n'
    ).encode("utf-8")


def add(archive: zipfile.ZipFile, name: str, payload: bytes, compress: int) -> None:
    info = zipfile.ZipInfo(name, STAMP)
    info.compress_type = compress
    info.external_attr = 0o100644 << 16
    archive.writestr(info, payload)


def build(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w") as archive:
        add(archive, "mimetype", MIMETYPE, zipfile.ZIP_STORED)
        add(archive, "content.xml", content(), zipfile.ZIP_DEFLATED)
        add(archive, "styles.xml", STYLES, zipfile.ZIP_DEFLATED)
        add(archive, "meta.xml", META, zipfile.ZIP_DEFLATED)
        add(archive, "META-INF/manifest.xml", MANIFEST, zipfile.ZIP_DEFLATED)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    expected = content()
    if args.check:
        try:
            with zipfile.ZipFile(OUTPUT) as archive:
                ok = archive.read("mimetype") == MIMETYPE and archive.read("content.xml") == expected
        except (OSError, KeyError, zipfile.BadZipFile):
            ok = False
        print("fixture ODS up to date" if ok else "fixture ODS stale")
        return 0 if ok else 1
    build(OUTPUT)
    print(hashlib.sha256(OUTPUT.read_bytes()).hexdigest())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
