"""Tests for common/tools/cua-image-manifest (python3 -m unittest discover)."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "..", "cua-image-manifest")
IMAGES = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
IMAGE_JSON = os.path.join(IMAGES, "linux", "image.json")
# Parsed by the Rust doctor's manifest tests (cua-spacesd-doctor), so the
# generator and the reader cannot drift.
RUST_FIXTURE = os.path.join(HERE, "fixtures", "manifest.linux.json")

BUILD_INFO = {
    "version": "0.1.0",
    "protocol_revision": 3,
    "git_sha": "0123456789abcdef",
    "cua_driver_version": "0.3.0",
    "tools_sha256": "ab" * 32,
    "tools_count": 42,
    "codecs_compiled": ["nvenc", "vaapi", "qsv", "amf", "openh264"],
}


def load_tool():
    loader = importlib.machinery.SourceFileLoader("cua_image_manifest", TOOL)
    spec = importlib.util.spec_from_loader("cua_image_manifest", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, TOOL, *args], capture_output=True, text=True)


class ManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.info = os.path.join(self.tmp.name, "build-info.json")
        with open(self.info, "w") as fh:
            json.dump(BUILD_INFO, fh)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def generate(self, variant: str = "rootfs", source: str = "local") -> dict:
        out = os.path.join(self.tmp.name, f"{variant}.json")
        args = [
            "generate", "--image-json", IMAGE_JSON, "--variant", variant, "--arch", "arm64",
            "--spacesd-source", source, "--source-revision", "deadbeef", "--out", out,
        ]
        if source != "none":
            args += ["--build-info", self.info]
        result = run(*args)
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(out) as fh:
            return json.load(fh)

    def test_rootfs_manifest_carries_claims_and_build_facts(self) -> None:
        m = self.generate()
        self.assertEqual(m["schema_version"], 1)
        self.assertEqual((m["name"], m["os"], m["variant"], m["init"]),
                         ("linux", "linux", "rootfs", "supervisord"))
        # The pre-rename name stays a read alias for one release.
        self.assertEqual(m["aliases"], ["cua-desktop-linux"])
        self.assertIn("desktop_stream", m["features_required"])
        self.assertIn("teleport.*", m["features_optional"])
        # The X11 desktop pins its presence cursor-shape backends.
        self.assertIn("presence.cursor_shape", m["features_required"])
        self.assertEqual(m["feature_attributes"]["presence.cursor_shape"],
                         {"hit_test": "atspi", "system": "xfixes", "probe": "xtest"})
        self.assertEqual(m["spacesd"]["version"], "0.1.0")
        self.assertEqual(m["spacesd"]["tools_sha256"], "ab" * 32)
        self.assertTrue(m["spacesd"]["present"])
        self.assertIn("cua-spacesd", m["units"])
        self.assertEqual(m["annotations"], {
            "ai.cua.image.os": "linux", "ai.cua.image.variant": "rootfs",
            "ai.cua.spacesd": "true", "ai.cua.env-driver": "true",
            # The image's default tier (full) unless --tier says otherwise.
            "ai.cua.image.tier": "full",
        })
        self.assertEqual(m["tier"], "full")
        # ssh is containerdisk-only.
        self.assertNotIn("ssh", [s["name"] for s in m["services"]])
        self.assertEqual(m["fixtures"]["apps"], ["grid", "form", "http", "tone", "avsync"])
        self.assertEqual(m["source_revision"], "deadbeef")

    def test_set_variant_rewrites_only_variant_fields(self) -> None:
        rootfs = self.generate()
        path = os.path.join(self.tmp.name, "rootfs.json")
        result = run("set-variant", "--in", path, "--image-json", IMAGE_JSON,
                     "--variant", "containerdisk")
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(path) as fh:
            disk = json.load(fh)
        self.assertEqual((disk["variant"], disk["init"]), ("containerdisk", "systemd"))
        self.assertIn("cua-spacesd.service", disk["units"])
        self.assertIn("ssh", [s["name"] for s in disk["services"]])
        self.assertEqual(disk["annotations"]["ai.cua.image.variant"], "containerdisk")
        for key in ("spacesd", "features_required", "compat_links", "source_revision"):
            self.assertEqual(disk[key], rootfs[key], key)

    def test_source_none_is_labelled_without_spacesd(self) -> None:
        m = self.generate(source="none")
        self.assertFalse(m["spacesd"]["present"])
        self.assertEqual(m["annotations"]["ai.cua.spacesd"], "false")
        self.assertEqual(m["annotations"]["ai.cua.env-driver"], "false")

    def test_present_spacesd_must_be_executable(self) -> None:
        result = run("generate", "--image-json", IMAGE_JSON, "--variant", "rootfs",
                     "--arch", "amd64", "--spacesd-source", "local",
                     "--spacesd", "/nonexistent/cua-spacesd")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not an executable", result.stderr)

    def test_check_labels_catches_a_mislabelled_image(self) -> None:
        self.generate(source="none")
        path = os.path.join(self.tmp.name, "rootfs.json")
        ok = run("check-labels", "--manifest", path, "--labels",
                 json.dumps({"ai.cua.spacesd": "false", "org.x": "y"}))
        self.assertEqual(ok.returncode, 0, ok.stderr)
        # The bug this guards: ai.cua.env-driver=true on a SOURCE=none build.
        bad = run("check-labels", "--manifest", path, "--labels",
                  json.dumps({"ai.cua.env-driver": "true"}))
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("ai.cua.env-driver", bad.stderr)

    def test_unknown_variant_is_refused(self) -> None:
        module = load_tool()
        with self.assertRaises(SystemExit):
            module.variant_fields({"claims": {}}, "floppy")

    def test_rust_fixture_is_current(self) -> None:
        """The Rust doctor parses tests/fixtures/manifest.linux.json;
        it must be exactly what this generator writes today, so the two cannot
        drift. Regenerate with CUA_UPDATE_FIXTURES=1."""
        m = self.generate()
        text = json.dumps(m, indent=2, sort_keys=True) + "\n"
        if os.environ.get("CUA_UPDATE_FIXTURES") == "1":
            os.makedirs(os.path.dirname(RUST_FIXTURE), exist_ok=True)
            with open(RUST_FIXTURE, "w") as fh:
                fh.write(text)
        with open(RUST_FIXTURE) as fh:
            self.assertEqual(fh.read(), text, "regenerate with CUA_UPDATE_FIXTURES=1")

    def test_stdout_output(self) -> None:
        module = load_tool()
        buf = io.StringIO()
        with redirect_stdout(buf):
            module.main(["generate", "--image-json", IMAGE_JSON, "--variant", "rootfs",
                         "--arch", "amd64", "--spacesd-source", "none"])
        self.assertEqual(json.loads(buf.getvalue())["arch"], "amd64")


TIERED = {
    "name": "tiered",
    "os": "linux",
    "outputs": {"rootfs": {"init": "supervisord"}, "containerdisk": {"init": "systemd"}},
    "claims": {
        "features_required": ["pty"],
        "units": {"supervisord": ["desktop"], "systemd": ["cua-desktop.service"]},
        "apps": {"chromium": ["chromium", "--version"], "python3": ["python3", "--version"]},
        "launch_app": {"app": "chromium", "args": []},
        "resources": {"min_disk_gib": 2, "min_memory_mib": 1024},
    },
    "default_tier": "full",
    "tiers": {
        "slim": {"description": "floor", "claims": {}},
        "full": {
            "from": "slim",
            "claims": {
                "apps": {"firefox": {"argv": ["firefox", "--version"], "expect": "^Mozilla Firefox"}},
                "tools": {"git": {"argv": ["git", "--version"], "expect": "^git version 2\\."}},
                "units": {"supervisord": ["desktop", "extra"]},
                "resources": {"min_disk_gib": 8},
            },
        },
        "xcode": {
            "from": "full",
            "claims": {
                "tools": {"xcodebuild": {"argv": ["xcodebuild", "-version"]}},
                "simulator_runtimes": ["iOS 26.0"],
                "launch_app": None,
            },
        },
        "loop": {"from": "loop"},
    },
}


class TierTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.image = os.path.join(self.tmp.name, "image.json")
        with open(self.image, "w") as fh:
            json.dump(TIERED, fh)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def generate(self, *extra: str) -> subprocess.CompletedProcess:
        return run("generate", "--image-json", self.image, "--variant", "rootfs",
                   "--arch", "arm64", "--spacesd-source", "none",
                   "--out", os.path.join(self.tmp.name, "m.json"), *extra)

    def manifest(self, *extra: str) -> dict:
        result = self.generate(*extra)
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(os.path.join(self.tmp.name, "m.json")) as fh:
            return json.load(fh)

    def test_default_tier_is_used_and_recorded(self) -> None:
        m = self.manifest()
        self.assertEqual(m["tier"], "full")
        self.assertEqual(m["annotations"]["ai.cua.image.tier"], "full")
        self.assertEqual(sorted(m["apps"]), ["chromium", "firefox", "python3"])
        self.assertEqual(m["tools"]["git"]["expect"], "^git version 2\\.")
        # Objects merge key by key; arrays replace.
        self.assertEqual(m["resources"], {"min_disk_gib": 8, "min_memory_mib": 1024})
        self.assertEqual(m["units"], ["desktop", "extra"])
        self.assertEqual(m["simulator_runtimes"], [])

    def test_slim_is_the_floor(self) -> None:
        m = self.manifest("--tier", "slim")
        self.assertEqual(m["tier"], "slim")
        self.assertEqual(sorted(m["apps"]), ["chromium", "python3"])
        self.assertEqual(m["tools"], {})
        self.assertEqual(m["units"], ["desktop"])

    def test_chain_and_null_deletes(self) -> None:
        m = self.manifest("--tier", "xcode")
        self.assertEqual(sorted(m["tools"]), ["git", "xcodebuild"])
        self.assertEqual(m["simulator_runtimes"], ["iOS 26.0"])
        self.assertIsNone(m["launch_app"])
        self.assertIn("firefox", m["apps"])

    def test_unknown_and_cyclic_tiers_are_refused(self) -> None:
        bad = self.generate("--tier", "huge")
        self.assertNotEqual(bad.returncode, 0)
        self.assertIn("unknown tier", bad.stderr)
        loop = self.generate("--tier", "loop")
        self.assertNotEqual(loop.returncode, 0)
        self.assertIn("inherits from itself", loop.stderr)

    def test_tier_without_tiers_is_refused(self) -> None:
        with open(IMAGE_JSON) as fh:
            image = json.load(fh)
        image.pop("tiers", None)
        image.pop("default_tier", None)
        untiered = os.path.join(self.tmp.name, "untiered.json")
        with open(untiered, "w") as fh:
            json.dump(image, fh)
        result = run("generate", "--image-json", untiered, "--variant", "rootfs",
                     "--arch", "arm64", "--spacesd-source", "none", "--tier", "slim")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no `tiers`", result.stderr)

    def test_set_variant_keeps_the_tier_claims(self) -> None:
        self.manifest("--tier", "full")
        path = os.path.join(self.tmp.name, "m.json")
        result = run("set-variant", "--in", path, "--image-json", self.image,
                     "--variant", "containerdisk")
        self.assertEqual(result.returncode, 0, result.stderr)
        with open(path) as fh:
            disk = json.load(fh)
        self.assertEqual(disk["tier"], "full")
        self.assertEqual(disk["annotations"]["ai.cua.image.tier"], "full")
        self.assertEqual(disk["units"], ["cua-desktop.service"])
        self.assertIn("git", disk["tools"])

    def test_helpers(self) -> None:
        module = load_tool()
        self.assertIsNone(module.pick_tier({"claims": {"a": 1}}, None))
        self.assertEqual(module.deep_merge({"a": {"b": 1, "c": 2}}, {"a": {"c": None, "d": 3}}),
                         {"a": {"b": 1, "d": 3}})


if __name__ == "__main__":
    unittest.main()
