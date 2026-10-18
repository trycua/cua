"""Profile measurement and rebuild-reuse contracts, with synthetic native command responses."""

import copy
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import profile_measure as measure
import profile_verify as verify
import test_profile_release as fixtures

GCC = "16.2.1 20260810"


class MeasureTest(fixtures.ProfileTest):
    schema = 2
    source_revision = "c" * 40
    driver_version = "0.31.0"

    def commands(self, runtime_path, compiler_runtime=None, owners=None):
        owners = owners or {"/usr/lib/libstdc++.so.6.0.99": "gcc-libs", "/usr/lib/libwayland-server.so.0": "wayland"}
        versions = {"hyprland": "hyprland 0.56.2-2", "gcc-libs": "gcc-libs 16.2.1-1", "wayland": "wayland 1.26.0-1"}

        def run(*command, input=None, extra_env=None):
            if command[0] == "ldd":
                return ("\tlibstdc++.so.6 => /usr/lib/libstdc++.so.6.0.99 (0x1)\n"
                        "\tlibwayland-server.so.0 => /usr/lib/libwayland-server.so.0 (0x2)")
            if command[:2] == ("pacman", "-Qoq"):
                return owners[command[2]]
            if command[:2] == ("pacman", "-Q"):
                return versions[command[2]]
            if "--print-file-name=libstdc++.so.6" in command:
                return compiler_runtime or "/usr/lib/libstdc++.so.6.0.99"
            if "-dM" in command:
                return f'#define __VERSION__ "{GCC}"'
            if "--modversion" in command:
                return "0.56.2"
            raise AssertionError(command)
        return run

    def run_measure(self, **kwargs):
        cxx = self.root / "g++"
        cxx.write_bytes(b"compiler")
        paths = {"/usr/lib/libstdc++.so.6.0.99": Path("/usr/lib/libstdc++.so.6.0.99"),
                 "/usr/lib/libwayland-server.so.0": Path("/usr/lib/libwayland-server.so.0")}
        digests = {"/usr/bin/Hyprland": "a" * 64, str(cxx): "b" * 64, "/usr/lib/libstdc++.so.6.0.99": "c" * 64}
        with mock.patch.object(verify, "run", side_effect=self.commands(None, **kwargs)), \
                mock.patch.object(verify, "digest", side_effect=lambda path: digests.get(str(path)) or verify.sha256(path.read_bytes())), \
                mock.patch.object(verify, "header_inventory_sha256", return_value="d" * 64), \
                mock.patch.object(verify, "verify_native") as native, \
                mock.patch.object(verify.platform, "system", return_value="Linux"), \
                mock.patch.object(verify.platform, "machine", return_value="x86_64"), \
                mock.patch.object(Path, "resolve", lambda self, strict=False: self):
            profile = measure.measure("synthetic-native", "1.0.0", 2, dict(self.profile["source"]), self.archive, cxx)
        native.assert_called_once()
        return profile

    def test_measured_profile_matches_reviewed_shape_and_source(self):
        profile = self.run_measure()
        self.assertEqual(profile, self.profile | {"runtime": {**self.profile["runtime"],
                         "packages": {"gcc-libs": "16.2.1-1", "wayland": "1.26.0-1"}}})

    def test_source_digest_substitution_refused(self):
        self.profile["source"]["archive_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "source archive checksum mismatch"):
            self.run_measure()

    def test_compiler_and_compositor_runtime_must_match(self):
        with self.assertRaisesRegex(ValueError, "different shared libstdc"):
            self.run_measure(compiler_runtime="/usr/lib/other/libstdc++.so.6.0.99")

    def test_reuse_rules(self):
        reviewed = verify.validate_profile(copy.deepcopy(self.profile))
        self.assertEqual(measure.reuse_decision(reviewed, copy.deepcopy(reviewed)), ("profile-unchanged", []))
        relabeled = copy.deepcopy(reviewed)
        relabeled["package_release"] = 3
        self.assertEqual(measure.reuse_decision(reviewed, relabeled), ("relabel-rebuild", ["package_release differs"]))
        for section, change in (("source", ("archive_sha256", "e" * 64)), ("hyprland", ("sha256", "e" * 64)),
                                ("compiler", ("sha256", "e" * 64)), ("runtime", ("sha256", "e" * 64))):
            changed = copy.deepcopy(reviewed)
            changed[section][change[0]] = change[1]
            self.assertEqual(measure.reuse_decision(reviewed, changed), ("rebuild", [f"{section} differs"]))
        dependency = copy.deepcopy(reviewed)
        dependency["runtime"]["packages"]["gcc-libs"] = "16.2.1-2"
        self.assertEqual(measure.reuse_decision(reviewed, dependency)[0], "rebuild")

    def test_reuse_refuses_invalid_profile(self):
        invalid = copy.deepcopy(self.profile)
        invalid["compiler"]["sha256"] = "short"
        with self.assertRaisesRegex(ValueError, "lowercase SHA-256"):
            measure.reuse_decision(self.profile, invalid)

    def test_measure_refuses_loader_override(self):
        with mock.patch.dict(os.environ, {"LD_LIBRARY_PATH": "/synthetic"}):
            with self.assertRaisesRegex(ValueError, "dynamic-loader"):
                self.run_measure()


if __name__ == "__main__":
    unittest.main()
