"""Standalone lifecycle gate tests; native/ALPM commands use synthetic fixtures."""

import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

from test_release import load


lifecycle = load("lifecycle")
release_fixture = load("test_release")


class KitTest(unittest.TestCase):
    setUp = release_fixture.ReleaseTest.setUp
    git = release_fixture.ReleaseTest.git
    write = release_fixture.ReleaseTest.write
    generate = release_fixture.ReleaseTest.generate

    def test_lifecycle_kit_requires_exact_source_and_checksums(self):
        kit = self.generate()
        manifest, checksums = lifecycle.verify_kit(kit, self.revision, "1.2.3")
        self.assertEqual(manifest["source_revision"], self.revision)
        self.assertIn("lifecycle.py", checksums)
        with self.assertRaises(ValueError):
            lifecycle.verify_kit(kit, "0" * 40, "1.2.3")
        (kit / "lifecycle.py").write_text("tampered runner")
        with self.assertRaisesRegex(ValueError, "kit checksum mismatch"):
            lifecycle.verify_kit(kit, self.revision, "1.2.3")

    def test_existing_build_and_checksum_traversal_refused(self):
        kit = self.generate()
        (kit / "src").mkdir()
        with self.assertRaisesRegex(ValueError, "fresh, unbuilt"):
            lifecycle.verify_kit(kit, self.revision, "1.2.3")
        (kit / "SHA256SUMS").write_text("0" * 64 + "  ../PKGBUILD\n")
        with self.assertRaisesRegex(ValueError, "unexpected or duplicate"):
            lifecycle.verify_kit(kit, self.revision, "1.2.3")

    def test_runner_copies_only_kit_files_and_marks_remaining_live_gates(self):
        kit = self.generate()
        output = kit / "evidence"
        compiler = self.root / "g++"
        compiler.write_text("synthetic compiler")
        original_read = Path.read_text

        def read_config(path, *args, **kwargs):
            if path == Path("/etc/makepkg.conf"):
                return "# synthetic makepkg configuration\n"
            return original_read(path, *args, **kwargs)

        def build(command, **kwargs):
            self.assertEqual(command[0], "makepkg")
            self.assertNotIn("--syncdeps", command)
            self.assertNotIn("--install", command)
            self.assertEqual(kwargs["env"]["CUA_RELEASE_CXX"], str(compiler))
            (kwargs["cwd"] / "cua-hyprland-plugin-1.2.3-1-x86_64.pkg.tar.zst").write_bytes(b"synthetic package")
            return subprocess.CompletedProcess(command, 0, "synthetic build", "")

        arguments = ["lifecycle.py", "--kit", str(kit), "--revision", self.revision,
                     "--driver-version", "1.2.3", "--cxx", str(compiler), "--output", str(output)]
        with mock.patch.object(sys, "argv", arguments), mock.patch.object(lifecycle.platform, "system", return_value="Linux"), \
                mock.patch.object(lifecycle.platform, "machine", return_value="x86_64"), \
                mock.patch.object(lifecycle.os, "geteuid", return_value=1000), \
                mock.patch.object(Path, "read_text", read_config), mock.patch.object(lifecycle, "run", side_effect=build), \
                mock.patch.object(lifecycle, "package_payload", return_value={}), \
                mock.patch.object(lifecycle, "qualify") as qualify, mock.patch("builtins.print"):
            lifecycle.main()
        qualify.assert_called_once()
        evidence = json.loads((output / "RESULT.json").read_text())
        self.assertEqual(evidence["source_revision"], self.revision)
        self.assertFalse(evidence["live_restart_verified"])
        self.assertFalse(evidence["live_rollback_verified"])
        self.assertFalse(evidence["published_release_verified"])


class LifecycleTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cua-lifecycle-test-")
        self.addCleanup(self.temporary.cleanup)
        self.work = Path(self.temporary.name)
        self.manifest = {"driver_version": "1.2.3", "source_revision": "a" * 40,
                         "files": {"LICENSE.md": lifecycle.digest(b"fixture license")}}
        module = b"synthetic module"
        build = {"source": self.manifest, "module_sha256": lifecycle.digest(module),
                 "module_runtime_sha256": "runtime", "compiler_runtime_sha256": "runtime",
                 "compositor_runtime_sha256": "runtime"}
        self.data = {lifecycle.MODULE: module, lifecycle.SOURCE: json.dumps(self.manifest).encode(),
                     lifecycle.BUILD: json.dumps(build).encode(),
                     f"usr/share/licenses/{lifecycle.PACKAGE}/LICENSE": b"fixture license"}
        self.payload = {name: lifecycle.digest(data) for name, data in self.data.items()}
        self.package = self.work / "candidate.pkg.tar.zst"
        self.package.write_text("synthetic package")
        self.installed = set()
        self.calls = []

    def fake_pacman(self, command, **kwargs):
        self.calls.append(command)
        self.assertEqual(command[:4], ["sudo", "-n", "--", "pacman"])
        root = Path(command[command.index("--root") + 1])
        self.assertIn(root, (self.work / "matching", self.work / "mismatched"))
        for option, suffix in (("--dbpath", "var/lib/pacman"), ("--config", "pacman.conf"),
                               ("--hookdir", "empty-hooks"), ("--cachedir", "cache"),
                               ("--logfile", "pacman.log")):
            self.assertEqual(command[command.index(option) + 1], str(root / suffix))
        self.assertIn("--noscriptlet", command)
        self.assertNotIn("--nodeps", command)
        operation = command[command.index("--noconfirm") + 1:]
        code, out, err = 0, "", ""
        if operation == ["-U", str(self.package)]:
            if root.name == "mismatched":
                code = 1
                out = ":: unable to satisfy dependency 'hyprland=0.56.2-1' required by cua-hyprland-plugin"
                err = "error: failed to prepare transaction (could not satisfy dependencies)"
            else:
                for name, data in self.data.items():
                    path = root / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(data)
                self.installed.add(root)
        elif operation[0] == "-R":
            for name in self.data:
                (root / name).unlink()
            self.installed.remove(root)
        elif operation[0] == "-Q":
            code = 0 if root in self.installed else 1
            out = f"{lifecycle.PACKAGE} 1.2.3-1\n" if code == 0 else ""
        return subprocess.CompletedProcess(command, code, out, err)

    def test_install_remove_reinstall_and_paired_dependency_refusal(self):
        with mock.patch.object(lifecycle, "run", side_effect=self.fake_pacman):
            lifecycle.qualify(self.work, self.package, self.payload, self.manifest)
        log = json.loads((self.work / "transactions.json").read_text())
        self.assertEqual([entry["operation"] for entry in log if entry["root"] == "matching"],
                         ["-U", "-U", "-Q", "-R", "-Q", "-U", "-Q"])
        self.assertEqual([entry["returncode"] for entry in log if entry["root"] == "mismatched"], [0, 1, 1])
        lifecycle.assert_state(self.work / "matching", self.payload, True)
        lifecycle.assert_state(self.work / "mismatched", self.payload, False)
        with tarfile.open(self.work / "hyprland-mismatched-fixture.pkg.tar.gz") as archive:
            self.assertEqual(archive.getnames(), [".PKGINFO"])
            self.assertIn(b"pkgver = 0.56.2-2", archive.extractfile(".PKGINFO").read())

    def test_generic_negative_control_failure_does_not_pass(self):
        def fake(command, **kwargs):
            result = self.fake_pacman(command, **kwargs)
            if result.returncode and "-U" in command:
                result.stdout = ""
                result.stderr = "permission denied"
            return result
        with mock.patch.object(lifecycle, "run", side_effect=fake):
            with self.assertRaisesRegex(ValueError, "specific Hyprland dependency refusal"):
                lifecycle.qualify(self.work, self.package, self.payload, self.manifest)

    def test_failed_positive_control_does_not_pass_and_retains_log(self):
        with mock.patch.object(lifecycle, "run", return_value=subprocess.CompletedProcess([], 1, "", "failure")):
            with self.assertRaisesRegex(ValueError, "pacman failed"):
                lifecycle.qualify(self.work, self.package, self.payload, self.manifest)
        self.assertEqual(json.loads((self.work / "transactions.json").read_text())[0]["returncode"], 1)

    def test_config_and_payload_drift_fail(self):
        root = lifecycle.new_root(self.work, "sentinel")
        for name, data in self.data.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        lifecycle.assert_state(root, self.payload, True)
        (root / lifecycle.MODULE).write_text("modified")
        with self.assertRaisesRegex(ValueError, "payload mismatch"):
            lifecycle.assert_state(root, self.payload, True)
        (root / "etc/hypr/hyprland.conf").write_text("changed config")
        with self.assertRaisesRegex(ValueError, "configuration changed"):
            lifecycle.assert_state(root, self.payload, True)

    def test_uninitialized_root_refused(self):
        with self.assertRaisesRegex(ValueError, "uninitialized"):
            lifecycle.pacman_command(Path("/"), "-U", str(self.package))

    def test_package_provenance_and_unexpected_config_or_hook_payload(self):
        names = list(self.payload) + [".PKGINFO", ".BUILDINFO", ".MTREE"]
        info = ("pkgname = cua-hyprland-plugin\npkgver = 1.2.3-1\narch = x86_64\n"
                "depend = hyprland=0.56.2-1\ndepend = gcc-libs\n")

        def fake_run(command, **kwargs):
            return subprocess.CompletedProcess(command, 0, "\n".join(names) if "-tf" in command else info, "")

        with mock.patch.object(lifecycle, "run", side_effect=fake_run), mock.patch.object(
                lifecycle.subprocess, "check_output", side_effect=lambda command: self.data[command[-1]]):
            self.assertEqual(lifecycle.package_payload(self.package, self.manifest), self.payload)
            for extra in (".INSTALL", "etc/hypr/hyprland.conf", "../escape"):
                names.append(extra)
                with self.assertRaisesRegex(ValueError, "unexpected package payload"):
                    lifecycle.package_payload(self.package, self.manifest)
                names.pop()
            self.data[lifecycle.MODULE] = b"tampered module"
            with self.assertRaisesRegex(ValueError, "module hash mismatch"):
                lifecycle.package_payload(self.package, self.manifest)


if __name__ == "__main__":
    unittest.main()
