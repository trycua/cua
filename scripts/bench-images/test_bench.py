#!/usr/bin/env python3
"""Offline tests for bench.py (no network, no docker): python3 test_bench.py"""

from __future__ import annotations

import hashlib
import io
import json
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench  # noqa: E402


def sample(**over) -> dict:
    b = {
        "schema_version": 1, "id": "demo", "name": "Demo", "version": "1.0", "kind": "image",
        "repository": "ghcr.io/trycua/bench-demo", "redistribution": "public", "visibility": "public",
        "license": {"code": "MIT"}, "upstream": {"repo": "https://example.com/demo"},
        "sources": [], "os": "linux", "arch": ["amd64", "arm64"], "variants": ["rootfs", "containerdisk"],
        "requires": {"rootfs": [], "containerdisk": ["kvm"]},
        "server": {"name": "demo", "port": 7000, "protocol": "http", "health": "GET /healthz"},
        "spacesd": True, "build": {"kind": "dockerfile"},
    }
    b.update(over)
    return b


class Validate(unittest.TestCase):
    def test_checked_in_benchmarks_are_valid(self):
        for i in bench.bench_ids():
            with self.subTest(i):
                self.assertEqual(bench.validate(json.loads((bench.BENCH_ROOT / i / "bench.json").read_text()), i), [])

    def test_sample_is_valid(self):
        self.assertEqual(bench.validate(sample()), [])

    def test_rules(self):
        cases = {
            "repository must be ghcr.io/trycua/bench-<id>": sample(repository="ghcr.io/trycua/linux"),
            "linux benchmarks ship both": sample(variants=["containerdisk"]),
            "VM-only": sample(os="windows", variants=["rootfs", "containerdisk"]),
            "reserved": sample(version="latest"),
            "plain tag word": sample(version="1.0-20260923-abcdef1"),
            "publishes private packages": sample(redistribution="private-only", visibility="public"),
            "unknown words": sample(requires={"rootfs": ["gpu"]}),
            "not a branch": sample(sources=[{"kind": "qcow2", "hf": {"repo": "a/b", "file": "x", "revision": "main"}, "sha256": "0" * 64}]),
            "64 hex": sample(sources=[{"kind": "qcow2", "url": "https://x/y", "sha256": "abc"}]),
        }
        for needle, doc in cases.items():
            with self.subTest(needle):
                problems = bench.validate(doc)
                self.assertTrue(any(needle in p for p in problems), problems)

    def test_windows_vm_only_ok(self):
        self.assertEqual(bench.validate(sample(os="windows", arch=["amd64"], variants=["containerdisk"],
                                               requires={"containerdisk": ["kvm"]})), [])


class Documents(unittest.TestCase):
    def test_labels(self):
        lab = bench.labels(sample(), "containerdisk", "20260923-abcdef1")
        self.assertEqual(lab["ai.cua.image.os"], "linux")
        self.assertEqual(lab["ai.cua.spacesd"], "true")
        self.assertEqual(lab["ai.cua.env-driver"], "true")
        self.assertEqual(json.loads(lab["ai.cua.image.requires"]), ["kvm"])
        self.assertEqual(json.loads(lab["ai.cua.bench.server"])["port"], 7000)
        self.assertEqual(lab["ai.cua.bench.id"], "demo")
        self.assertEqual(lab["org.opencontainers.image.revision"], "abcdef1")

    def test_private_benchmarks_never_link_to_the_public_repo(self):
        # ghcr gives a package linked to a public repository public visibility.
        self.assertIn(bench.SOURCE_LABEL, bench.labels(sample(), "rootfs"))
        private = sample(redistribution="private-only", visibility="private")
        self.assertNotIn(bench.SOURCE_LABEL, bench.labels(private, "rootfs"))
        disk_only, _ = bench.build_indexes(private, "20260923-abcdef1", [], [])
        self.assertNotIn(bench.SOURCE_LABEL, disk_only["annotations"])

    def test_indexes(self):
        b = sample()
        mk = lambda d, a: {"mediaType": bench.OCI_MANIFEST, "digest": d, "size": 1,
                           "platform": {"architecture": a, "os": "linux"}}
        rootfs = [mk("sha256:r1", "amd64"), mk("sha256:r2", "arm64")]
        disks = [mk("sha256:d1", "amd64"), mk("sha256:d2", "arm64")]
        disk_index, rootfs_index = bench.build_indexes(b, "20260923-abcdef1", rootfs, disks)
        self.assertIsNone(rootfs_index)  # needs the pushed disk index's digest
        # One variant per index: docker/containerd take the first platform match.
        self.assertEqual([m["digest"] for m in disk_index["manifests"]], ["sha256:d1", "sha256:d2"])
        self.assertTrue(all("variant" not in m["platform"] for m in disk_index["manifests"]))
        self.assertEqual(json.loads(disk_index["annotations"]["ai.cua.image.variants"]),
                         {"rootfs": "ghcr.io/trycua/bench-demo:1.0-20260923-abcdef1"})
        self.assertEqual(disk_index["annotations"]["ai.cua.image.variant"], "containerdisk")
        self.assertEqual(json.loads(disk_index["annotations"]["ai.cua.image.requires"]), ["kvm"])
        _, rootfs_index = bench.build_indexes(b, "20260923-abcdef1", rootfs, disks, "sha256:dd")
        self.assertEqual([m["digest"] for m in rootfs_index["manifests"]], ["sha256:r1", "sha256:r2"])
        self.assertEqual({m["annotations"]["ai.cua.image.variant"] for m in rootfs_index["manifests"]},
                         {"rootfs"})
        self.assertEqual(json.loads(rootfs_index["annotations"]["ai.cua.image.variants"]),
                         {"containerdisk": "ghcr.io/trycua/bench-demo@sha256:dd"})
        for k in ("ai.cua.image.os", "ai.cua.spacesd", "ai.cua.bench.id", "ai.cua.bench.version",
                  "ai.cua.bench.server", "ai.cua.image.requires"):
            self.assertIn(k, rootfs_index["annotations"])

    def test_vm_only_has_no_rootfs_index(self):
        b = sample(os="windows", arch=["amd64"], variants=["containerdisk"], requires={"containerdisk": ["kvm"]})
        mk = {"mediaType": bench.OCI_MANIFEST, "digest": "sha256:d", "size": 1,
              "platform": {"architecture": "amd64", "os": "linux"}}
        disk_index, rootfs_index = bench.build_indexes(b, "20260923-abcdef1", [], [mk], "sha256:x")
        self.assertIsNone(rootfs_index)
        self.assertNotIn("ai.cua.image.variants", disk_index["annotations"])


class Layers(unittest.TestCase):
    def _tar(self, path: Path, entries: dict[str, bytes | None]) -> None:
        with tarfile.open(path, "w") as t:
            for name, data in entries.items():
                ti = tarfile.TarInfo(name)
                if data is None:
                    ti.type = tarfile.DIRTYPE
                    t.addfile(ti)
                else:
                    ti.size = len(data)
                    t.addfile(ti, io.BytesIO(data))

    def test_split_tar(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            self._tar(d / "in.tar", {"./usr": None, "./usr/bin/x": b"x", "./home/user/f": b"f",
                                     "./boot": None, "./boot/vmlinuz": b"k", "./etc/fstab": b"s", "./etc/a": b"a"})
            metas = bench.split_tar(d / "in.tar", d / "out", [("usr", ["usr"]), ("home", ["home"])],
                                    ["boot", "etc/fstab"])
            names = {}
            for m in metas:
                raw = Path(m["path"]).read_bytes()
                self.assertEqual(m["digest"], "sha256:" + hashlib.sha256(raw).hexdigest())
                with tarfile.open(m["path"]) as t:
                    names[Path(m["path"]).name] = sorted(t.getnames())
                import gzip
                self.assertEqual(m["diff_id"], "sha256:" + hashlib.sha256(gzip.decompress(raw)).hexdigest())
            self.assertEqual(names["00-usr.tar.gz"], ["usr", "usr/bin/x"])
            self.assertEqual(names["01-home.tar.gz"], ["home/user/f"])
            self.assertEqual(names["02-rest.tar.gz"], ["boot", "etc/a"])  # mount point kept, content dropped

    def test_disk_layer_and_layout(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "disk.img").write_bytes(b"QFI\xfb" + b"\0" * 1000)
            meta = bench.disk_layer(d / "disk.img", d / "disk.tar")
            self.assertEqual(meta["digest"], meta["diff_id"])
            with tarfile.open(d / "disk.tar") as t:
                m = t.getmember("disk/disk.img")
                self.assertEqual((m.uid, m.gid), (107, 107))
            desc = bench.write_layout(d / "layout", "linux/arm64", {"config": {"Labels": {"a": "b"}}}, [meta])
            man = json.loads((d / "layout/blobs/sha256" / desc["digest"][7:]).read_text())
            cfg = json.loads((d / "layout/blobs/sha256" / man["config"]["digest"][7:]).read_text())
            self.assertEqual(cfg["architecture"], "arm64")
            self.assertEqual(cfg["rootfs"]["diff_ids"], [meta["diff_id"]])
            self.assertTrue((d / "layout/blobs/sha256" / meta["digest"][7:]).is_file())
            idx = json.loads((d / "layout/index.json").read_text())
            self.assertEqual(idx["manifests"][0]["digest"], desc["digest"])


if __name__ == "__main__":
    unittest.main(verbosity=1)
