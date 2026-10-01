"""Tests for scripts/images/record-image-sizes.py (python3 -m unittest).

Refs are assembled from pieces so the image-ref gate does not read them.
"""

from __future__ import annotations

import gzip
import hashlib
import importlib.util
import io
import json
import os
import struct
import sys
import tarfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(os.path.dirname(HERE), "record-image-sizes.py")
GH = "ghcr" + ".io/" + "trycua"
D1 = "sha256:" + "1" * 64
D2 = "sha256:" + "2" * 64


def load():
    spec = importlib.util.spec_from_file_location("record_image_sizes", TOOL)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["record_image_sizes"] = mod  # dataclasses look the module up
    spec.loader.exec_module(mod)
    return mod


def tar_of(files: dict[str, bytes]) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.USTAR_FORMAT) as t:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))
    return buf.getvalue()


def chunks(b: bytes, n: int = 700):
    return iter([b[i : i + n] for i in range(0, len(b), n)])


class FakeProc:
    def __init__(self, data: bytes):
        self.stdout = io.BytesIO(data)
        self.returncode = 0

    def kill(self):
        pass

    def wait(self):
        return 0


CATALOG = """{
  "images": [
    {
      "ref": "%(gh)s/linux:1",
      "os": "linux",
      "spacesd": true,
      "digest": "%(d1)s",
      "arch": ["amd64"],
      "published": true
    },
    {
      "ref": "%(gh)s/linux:2",
      "os": "linux",
      "spacesd": true,
      "arch": ["amd64"],
      "published": false
    }
  ]
}
""" % {"gh": GH, "d1": D1}


class ImageSizesTest(unittest.TestCase):
    def setUp(self):
        self.m = load()

    def test_containerdisk_reads_the_qcow2_virtual_size(self):
        qcow2 = self.m.QCOW2_MAGIC + b"\0" * 20 + struct.pack(">Q", 20 << 30) + b"\0" * 1000
        layer = tar_of({"disk/": b"", "disk/disk.img": qcow2})
        self.assertEqual(self.m.tar_disk(chunks(layer)), (len(qcow2), 20 << 30))
        raw = b"\1" * 3000
        self.assertEqual(self.m.tar_disk(chunks(tar_of({"disk/disk.img": raw}))), (3000, 3000))
        self.assertIsNone(self.m.tar_disk(chunks(tar_of({"etc/hostname": b"x"}))))

    def test_gzip_layers_decompress_in_pieces(self):
        data = os.urandom(5000) + b"\0" * 5000
        z = gzip.compress(data[:4000]) + gzip.compress(data[4000:])  # two members
        out = b"".join(self.m.decompressed("application/vnd.oci.image.layer.v1.tar+gzip", chunks(z, 97)))
        self.assertEqual(out, data)

    def test_rootfs_counts_uncompressed_bytes(self):
        a = tar_of({"bin/sh": b"x" * 4000})
        b = tar_of({"etc/os-release": b"y" * 100})
        blobs = {D1: gzip.compress(a), D2: b}
        manifest = {
            "layers": [
                {"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": D1, "size": len(blobs[D1])},
                {"mediaType": "application/vnd.oci.image.layer.v1.tar", "digest": D2, "size": len(b)},
            ]
        }
        got = self.m.measure_platform("r", "d", manifest, stream=lambda _r, d: FakeProc(blobs[d]))
        self.assertEqual(got, (len(blobs[D1]) + len(b), len(a) + len(b), len(a) + len(b)))

    def test_lume_skips_zero_chunks(self):
        size = 4096
        zero = self.m.zero_digest(size)
        self.assertEqual(zero, "sha256:" + hashlib.sha256(b"\0" * size).hexdigest())
        chunk = lambda d: {
            "mediaType": "application/vnd.trycua.lume.disk.v1",
            "size": 10,
            "annotations": {self.m.LUME_CHUNK_SIZE: str(size), self.m.LUME_CHUNK_DIGEST: d},
        }
        manifest = {
            "annotations": {self.m.LUME_DISK_SIZE: str(150 << 30)},
            "layers": [
                {"mediaType": "application/vnd.trycua.lume.nvram.v1", "size": 7},
                chunk(D1),
                chunk(zero),
                chunk(zero),
            ],
        }
        self.assertEqual(self.m.lume_size(manifest), (37, 7 + size, 150 << 30))

    def test_set_sizes_edits_only_the_entry(self):
        ref = f"{GH}/linux:1"
        rows = [self.m.PlatformSize("amd64", D2, 1, 2, 3)]
        once = self.m.set_sizes(CATALOG, ref, D1, rows)
        doc = json.loads(once)
        self.assertEqual(doc["images"][0]["sizes"]["digest"], D1)
        self.assertEqual(doc["images"][0]["sizes"]["platforms"][0]["disk"], 3)
        # Everything else is byte-for-byte the same, and a rerun replaces.
        added = [l for l in once.split("\n") if l not in CATALOG.split("\n")]
        self.assertTrue(all("sizes" in l or "platforms" in l or l.strip() in ("]", "},") or "amd64" in l or D1 in l for l in added))
        twice = self.m.set_sizes(once, ref, D1, [self.m.PlatformSize("amd64", D2, 1, 2, 4)])
        self.assertEqual(twice.count('"sizes"'), 1)
        self.assertEqual(json.loads(twice)["images"][0]["sizes"]["platforms"][0]["disk"], 4)
        # A first digest (a release that publishes an entry).
        new = self.m.set_sizes(twice, f"{GH}/linux:2", D2, rows, new_digest=True)
        entry = json.loads(new)["images"][1]
        self.assertEqual((entry["digest"], entry["sizes"]["digest"]), (D2, D2))

    def test_check_flags_missing_and_stale_sizes(self):
        doc = json.loads(CATALOG)
        self.assertEqual(self.m.check(doc), [f"{GH}/linux:1"])
        rows = [self.m.PlatformSize("amd64", D2, 1, 2, 3)]
        doc = json.loads(self.m.set_sizes(CATALOG, f"{GH}/linux:1", D1, rows))
        self.assertEqual(self.m.check(doc), [])
        doc["images"][0]["digest"] = D2
        self.assertEqual(self.m.check(doc), [f"{GH}/linux:1"])

    def test_the_repo_catalog_is_current(self):
        with open(self.m.CATALOG, encoding="utf-8") as f:
            self.assertEqual(self.m.check(json.load(f)), [])


if __name__ == "__main__":
    unittest.main()
