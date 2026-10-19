#!/usr/bin/env python3
"""Record how big each catalog image is, from its published manifests.

    scripts/images/record-image-sizes.py [--check] [--force] [--ref REF ...]
        [--refresh-digest] [--catalog FILE]

For every entry of ``libs/images/sandbox-images.json`` with a ``digest``,
writes ``sizes``: the digest they were measured at and one row per platform
(attestation manifests are skipped):

    "sizes": {
      "digest": "sha256:...",
      "platforms": [
        { "arch": "arm64", "manifest": "sha256:...", "download": N, "unpacked": N, "disk": N }
      ]
    }

* ``manifest``: the platform manifest's digest (what the QEMU image cache
  keys a pulled containerDisk by).
* ``download``: the compressed bytes a pull transfers (every layer).
* ``unpacked``: the bytes the pulled image takes on the host: a container's
  uncompressed layers, a containerDisk's ``disk.img`` file, or a Lume disk's
  non-zero chunks (an upper bound: Lume writes zero chunks as holes).
* ``disk``: the disk the Space sees: a VM's virtual disk size (the qcow2
  header, or the Lume ``org.trycua.lume.disk-size`` annotation); a
  container's unpacked root filesystem.

Only stale entries are measured (``sizes.digest`` differs from ``digest``),
so a run after a republish costs one pass over the new images. ``--ref``
limits the run to those refs; ``--force`` re-measures them anyway;
``--refresh-digest`` first moves each named ref's ``digest`` to what its tag
resolves to now (the release pipeline's ``catalog`` step, after promote).
``--check`` is offline: it fails when an entry with a digest has no sizes
or sizes measured at another digest (CI runs it, so a digest bump without
new sizes cannot land).

The file is edited in place, line by line, so every other line keeps its
formatting. Needs ``crane`` (and ``zstd`` for zstd layers) on PATH.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import struct
import subprocess
import sys
import zlib
from dataclasses import dataclass
from typing import Any, Callable, Iterable, Iterator

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CATALOG = os.path.join(ROOT, "libs", "images", "sandbox-images.json")

LUME_PREFIX = "application/vnd.trycua.lume."
LUME_DISK_SIZE = "org.trycua.lume.disk-size"
LUME_CHUNK_SIZE = "org.trycua.lume.content.uncompressed-size"
LUME_CHUNK_DIGEST = "org.trycua.lume.content.uncompressed-digest"
CONTAINER_DISK_PATH = "disk/disk.img"
QCOW2_MAGIC = b"QFI\xfb"
READ = 1 << 20


def fail(message: str) -> None:
    print(f"record-image-sizes: {message}", file=sys.stderr)
    sys.exit(1)


@dataclass
class PlatformSize:
    arch: str
    manifest: str
    download: int
    unpacked: int
    disk: int

    def line(self) -> str:
        return (
            f'{{ "arch": "{self.arch}", "manifest": "{self.manifest}", '
            f'"download": {self.download}, "unpacked": {self.unpacked}, "disk": {self.disk} }}'
        )


# ------------------------------------------------------------ measuring


def repo_of(ref: str) -> str:
    """``ghcr.io/trycua/linux:24.04`` -> ``ghcr.io/trycua/linux``."""
    name = ref.split("@", 1)[0]
    slash = name.rfind("/")
    colon = name.rfind(":")
    return name[:colon] if colon > slash else name


def crane_json(*args: str) -> Any:
    return json.loads(subprocess.check_output(["crane", *args]))


def blob_stream(repo: str, digest: str) -> subprocess.Popen:
    return subprocess.Popen(
        ["crane", "blob", f"{repo}@{digest}"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    )


def decompressed(media_type: str, chunks: Iterable[bytes]) -> Iterator[bytes]:
    """The layer's tar bytes, whatever its compression."""
    if media_type.endswith("zstd"):
        proc = subprocess.Popen(["zstd", "-dc"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        import threading

        def feed() -> None:
            assert proc.stdin
            for c in chunks:
                proc.stdin.write(c)
            proc.stdin.close()

        t = threading.Thread(target=feed, daemon=True)
        t.start()
        assert proc.stdout
        while True:
            b = proc.stdout.read(READ)
            if not b:
                break
            yield b
        t.join()
        proc.wait()
        return
    if media_type.endswith("gzip"):
        d = zlib.decompressobj(zlib.MAX_WBITS | 32)
        for c in chunks:
            while c:
                out = d.decompress(c)
                if out:
                    yield out
                # A multi-member gzip: start a new member on the leftover.
                if d.eof:
                    c = d.unused_data
                    d = zlib.decompressobj(zlib.MAX_WBITS | 32)
                else:
                    c = b""
        tail = d.flush()
        if tail:
            yield tail
        return
    yield from chunks


def proc_chunks(proc: subprocess.Popen) -> Iterator[bytes]:
    assert proc.stdout
    while True:
        b = proc.stdout.read(READ)
        if not b:
            return
        yield b


def tar_disk(stream: Iterator[bytes]) -> tuple[int, int] | None:
    """``(file size, virtual size)`` of ``disk/disk.img`` in a tar stream,
    reading only up to the image's header. ``None``: not in this layer."""
    buf = b""

    def need(n: int) -> bool:
        nonlocal buf
        while len(buf) < n:
            try:
                buf += next(stream)
            except StopIteration:
                return False
        return True

    while need(512):
        header, buf = buf[:512], buf[512:]
        if header == b"\0" * 512:
            return None
        name = header[0:100].rstrip(b"\0").decode("utf-8", "replace")
        prefix = header[345:500].rstrip(b"\0").decode("utf-8", "replace")
        if prefix and header[257:262] == b"ustar":
            name = f"{prefix}/{name}"
        size_field = header[124:136]
        if size_field[0] & 0x80:  # base-256 for files of 8 GiB and more
            size = int.from_bytes(size_field[1:], "big")
        else:
            size = int(size_field.rstrip(b"\0 ").decode() or "0", 8)
        if name.lstrip("./") == CONTAINER_DISK_PATH:
            if not need(32):
                return None
            if buf[:4] == QCOW2_MAGIC:
                return size, struct.unpack(">Q", buf[24:32])[0]
            return size, size
        skip = (size + 511) // 512 * 512
        while skip:
            if not buf and not need(1):
                return None
            n = min(skip, len(buf))
            buf, skip = buf[n:], skip - n
    return None


_ZERO: dict[int, str] = {}


def zero_digest(size: int) -> str:
    """sha256 of ``size`` zero bytes (a Lume chunk that is all holes)."""
    if size not in _ZERO:
        h = hashlib.sha256()
        block = b"\0" * READ
        left = size
        while left:
            n = min(left, READ)
            h.update(block[:n])
            left -= n
        _ZERO[size] = "sha256:" + h.hexdigest()
    return _ZERO[size]


def lume_size(manifest: dict[str, Any]) -> tuple[int, int, int]:
    """``(download, unpacked, disk)`` of a Lume manifest."""
    layers = manifest.get("layers", [])
    download = sum(l["size"] for l in layers)
    annotations = manifest.get("annotations", {})
    unpacked = 0
    for l in layers:
        a = l.get("annotations", {})
        if LUME_CHUNK_SIZE in a:
            size = int(a[LUME_CHUNK_SIZE])
            if a.get(LUME_CHUNK_DIGEST) != zero_digest(size):
                unpacked += size
        else:
            unpacked += l["size"]
    disk = int(annotations.get(LUME_DISK_SIZE) or annotations.get("org.trycua.lume.total-uncompressed-size") or 0)
    if not disk:
        raise ValueError("Lume manifest without org.trycua.lume.disk-size")
    return download, unpacked, disk


def measure_platform(
    repo: str,
    digest: str,
    manifest: dict[str, Any],
    stream: Callable[[str, str], subprocess.Popen] = blob_stream,
) -> tuple[int, int, int]:
    """``(download, unpacked, disk)`` of one platform manifest."""
    layers = manifest.get("layers", [])
    if any(l["mediaType"].startswith(LUME_PREFIX) for l in layers):
        return lume_size(manifest)
    download = sum(l["size"] for l in layers)
    # A containerDisk: the disk is normally in the last layer.
    for l in reversed(layers):
        proc = stream(repo, l["digest"])
        try:
            found = tar_disk(decompressed(l["mediaType"], proc_chunks(proc)))
        finally:
            proc.kill()
            proc.wait()
        if found:
            return download, found[0], found[1]
        break  # no disk in the top layer: a container rootfs
    unpacked = 0
    for l in layers:
        if l["mediaType"].endswith(".tar"):
            unpacked += l["size"]
            continue
        proc = stream(repo, l["digest"])
        try:
            for b in decompressed(l["mediaType"], proc_chunks(proc)):
                unpacked += len(b)
        finally:
            proc.wait()
        if proc.returncode:
            raise RuntimeError(f"crane blob {repo}@{l['digest']} failed")
    return download, unpacked, unpacked


def measure(ref: str, digest: str) -> list[PlatformSize]:
    repo = repo_of(ref)
    top = crane_json("manifest", f"{repo}@{digest}")
    rows: list[PlatformSize] = []
    if "manifests" in top:
        for child in top["manifests"]:
            p = child.get("platform", {})
            arch = p.get("architecture", "unknown")
            if arch == "unknown" or p.get("os") == "unknown":
                continue  # attestations
            m = crane_json("manifest", f"{repo}@{child['digest']}")
            d, u, k = measure_platform(repo, child["digest"], m)
            rows.append(PlatformSize(arch, child["digest"], d, u, k))
    else:
        # A single manifest: Lume images are Apple silicon only.
        lume = any(l["mediaType"].startswith(LUME_PREFIX) for l in top.get("layers", []))
        arch = "arm64" if lume else crane_json("config", f"{repo}@{digest}").get("architecture", "amd64")
        d, u, k = measure_platform(repo, digest, top)
        rows.append(PlatformSize(arch, digest, d, u, k))
    rows.sort(key=lambda r: r.arch)
    return rows


# ------------------------------------------------------------ the file


def entry_span(lines: list[str], ref: str) -> tuple[int, int]:
    """Line range ``[start, end)`` of the image object whose ref is ``ref``."""
    key = f'"ref": {json.dumps(ref)},'
    for i, line in enumerate(lines):
        if line.strip() == key:
            start = i
            while not lines[start].rstrip().endswith("{"):
                start -= 1
            indent = len(lines[start]) - len(lines[start].lstrip())
            end = i
            while not (
                lines[end].strip() in ("}", "},")
                and len(lines[end]) - len(lines[end].lstrip()) == indent
            ):
                end += 1
            return start, end + 1
    raise KeyError(ref)


def sizes_lines(digest: str, rows: list[PlatformSize], indent: str) -> list[str]:
    inner = indent + "  "
    out = [f'{indent}"sizes": {{', f'{inner}"digest": "{digest}",', f'{inner}"platforms": [']
    for i, r in enumerate(rows):
        out.append(f"{inner}  {r.line()}" + ("," if i < len(rows) - 1 else ""))
    out += [f"{inner}]", f"{indent}}},"]
    return out


def set_sizes(text: str, ref: str, digest: str, rows: list[PlatformSize], new_digest: bool = False) -> str:
    """``text`` with ``ref``'s ``sizes`` (and, with ``new_digest``, its
    ``digest``) replaced; every other line is kept as is."""
    lines = text.split("\n")
    start, end = entry_span(lines, ref)
    body = lines[start:end]
    # Drop an existing sizes block.
    for i, line in enumerate(body):
        if line.strip() == '"sizes": {':
            depth, j = 0, i
            while True:
                depth += body[j].count("{") + body[j].count("[")
                depth -= body[j].count("}") + body[j].count("]")
                if depth == 0:
                    break
                j += 1
            del body[i : j + 1]
            break
    at = next((i for i, l in enumerate(body) if l.strip().startswith('"digest":')), None)
    if at is None:
        if not new_digest:
            raise ValueError(f"{ref}: no digest line")
        # A first digest goes after `spacesd` (where the others have it).
        after = next(i for i, l in enumerate(body) if l.strip().startswith('"spacesd":'))
        body.insert(after + 1, body[after][: len(body[after]) - len(body[after].lstrip())] + '"digest": "",')
        at = after + 1
    indent = body[at][: len(body[at]) - len(body[at].lstrip())]
    if new_digest:
        body[at] = f'{indent}"digest": "{digest}",'
    body[at + 1 : at + 1] = sizes_lines(digest, rows, indent)
    lines[start:end] = body
    return "\n".join(lines)


def stale(image: dict[str, Any]) -> bool:
    digest = image.get("digest")
    if not digest:
        return False
    s = image.get("sizes") or {}
    return s.get("digest") != digest or not s.get("platforms")


def check(catalog: dict[str, Any]) -> list[str]:
    """Entries whose sizes are missing or measured at another digest."""
    return [i["ref"] for i in catalog["images"] if stale(i)]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--catalog", default=CATALOG)
    ap.add_argument("--check", action="store_true", help="offline: fail on missing or stale sizes")
    ap.add_argument("--ref", action="append", default=[], help="only these refs (repeatable)")
    ap.add_argument("--force", action="store_true", help="re-measure even when current")
    ap.add_argument(
        "--refresh-digest",
        action="store_true",
        help="move each --ref's digest to what its tag resolves to now",
    )
    a = ap.parse_args(argv)
    with open(a.catalog, encoding="utf-8") as f:
        text = f.read()
    catalog = json.loads(text)
    if a.check:
        bad = check(catalog)
        if bad:
            print(
                "record-image-sizes: sizes missing or measured at another digest for "
                + ", ".join(bad)
                + "; run python3 scripts/images/record-image-sizes.py",
                file=sys.stderr,
            )
            return 1
        print(f"record-image-sizes: sizes current for every image with a digest")
        return 0
    if a.refresh_digest and not a.ref:
        fail("--refresh-digest needs --ref")
    known = {i["ref"]: i for i in catalog["images"]}
    for r in a.ref:
        if r not in known:
            fail(f"{r} is not in the catalog")
    todo = [known[r] for r in a.ref] if a.ref else list(catalog["images"])
    changed = 0
    for image in todo:
        ref = image["ref"]
        digest = image.get("digest")
        moved = False
        if a.refresh_digest:
            now = subprocess.check_output(["crane", "digest", ref], text=True).strip()
            moved = now != digest
            digest = now
        if not digest:
            continue
        if not (a.force or moved or stale(image)):
            continue
        print(f"measuring {ref}@{digest}", file=sys.stderr)
        rows = measure(ref, digest)
        text = set_sizes(text, ref, digest, rows, new_digest=moved)
        changed += 1
        with open(a.catalog, "w", encoding="utf-8") as f:
            f.write(text)
    json.loads(text)
    print(f"record-image-sizes: {changed} image(s) measured")
    return 0


if __name__ == "__main__":
    sys.exit(main())
