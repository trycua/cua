"""Profile packaging contracts, with synthetic archives and native command responses."""

import copy
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

import profile_bundle as bundle
import profile_verify as verify
import lifecycle

HERE = Path(__file__).resolve().parent


class ProfileTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cua-profile-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tooling_sha = "b" * 40
        self.files = {"CMakeLists.txt": b"project(cua_hyprland_plugin VERSION 0.1.0 LANGUAGES CXX)\n",
                      "LICENSE.md": b"Synthetic license\n", "verify.py": b"raise SystemExit('historical verifier must never execute')\n",
                      "src/plugin.cpp": b"// synthetic source\n"}
        self.manifest = {"schema": 1, "source_revision": verify.SOURCE_REVISION, "driver_version": "0.24.0",
                         "release_tag": "cua-driver-rs-v0.24.0", "plugin_version": "0.1.0", "architecture": "x86_64",
                         "native_certified": False, "cmake_options": verify.OPTIONS, "hyprland_version": "0.56.2",
                         "hyprland_package": "0.56.2-1", "compiler_version": "16.1.1 20260728",
                         "compiler_comment": "GCC: (GNU) 16.1.1 20260728",
                         "files": {name: verify.sha256(data) for name, data in self.files.items()}}
        self.files["SOURCE-PROVENANCE.json"] = verify.json_bytes(self.manifest)
        self.archive = self.root / (verify.STEM + ".tar.gz")
        self.archive.write_bytes(bundle.deterministic_archive({verify.STEM + "/" + name: data for name, data in self.files.items()}))
        self.profile = {"schema": 1, "profile_id": "synthetic-native", "kit_version": "1.0.0", "package_release": 2,
                        "architecture": "x86_64", "source": {"revision": verify.SOURCE_REVISION, "driver_version": "0.24.0",
                        "archive_sha256": verify.digest(self.archive), "manifest_sha256": verify.sha256(self.files["SOURCE-PROVENANCE.json"])},
                        "hyprland": {"package_version": "0.56.2-2", "header_version": "0.56.2", "headers_sha256": "d" * 64, "sha256": "a" * 64},
                        "compiler": {"version": "16.2.1 20260810", "comment": "GCC: (GNU) 16.2.1 20260810", "sha256": "b" * 64},
                        "runtime": {"basename": "libstdc++.so.6.0.99", "sha256": "c" * 64, "packages": {"gcc-libs": "16.2.1-1"}}}
        self.profile_path = self.root / "reviewed.json"
        self.profile_path.write_bytes(verify.json_bytes(self.profile))

    def generate(self, name="output"):
        output = self.root / name
        with mock.patch.object(bundle, "committed_file", side_effect=lambda repo, sha, name: (HERE / name).read_bytes()) as committed, mock.patch.object(bundle.subprocess, "check_output", return_value=self.tooling_sha + "\n"):
            metadata = bundle.generate(self.root, self.tooling_sha, self.profile_path, self.archive, output)
        self.assertTrue(all(call.args[1] == self.tooling_sha for call in committed.call_args_list))
        archive = next(output.glob("*.tar.gz"))
        kit = self.root / (name + "-kit")
        kit.mkdir()
        with tarfile.open(archive) as contents:
            contents.extractall(kit, filter="data")
        return output, kit, metadata

    def source(self):
        source = self.root / "source"
        source.mkdir()
        for name, data in self.files.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        return source

    def test_deterministic_separate_kit_and_unchanged_archive(self):
        output, kit, metadata = self.generate()
        second, _, _ = self.generate("second")
        self.assertEqual({p.name: p.read_bytes() for p in output.iterdir()}, {p.name: p.read_bytes() for p in second.iterdir()})
        self.assertEqual((kit / self.archive.name).read_bytes(), self.archive.read_bytes())
        self.assertEqual((kit / "SOURCE-PROVENANCE.json").read_bytes(), self.files["SOURCE-PROVENANCE.json"])
        self.assertFalse(metadata["native_certified"])
        self.assertEqual(metadata["source"]["revision"], verify.SOURCE_REVISION)
        self.assertNotEqual(metadata["source"]["revision"], metadata["tooling_revision"])
        verify.verify_kit(kit, verify.digest(kit / "KIT-PROVENANCE.json"), complete=True)
        subprocess.run(["bash", "-n", str(kit / "PKGBUILD")], check=True)
        subprocess.run(["shasum", "-a", "256", "-c", "SHA256SUMS"], cwd=kit, capture_output=True, check=True)

    def test_profile_change_changes_kit_but_not_source(self):
        first, kit, _ = self.generate()
        self.profile["package_release"] = 3
        self.profile_path.write_bytes(verify.json_bytes(self.profile))
        second, other, _ = self.generate("other")
        self.assertNotEqual(next(first.glob("*.tar.gz")).name, next(second.glob("*.tar.gz")).name)
        self.assertEqual((kit / self.archive.name).read_bytes(), (other / self.archive.name).read_bytes())

    def test_generation_refuses_wrong_source_dirty_tooling_and_existing_output(self):
        self.generate()
        with mock.patch.object(bundle, "committed_file", side_effect=lambda repo, sha, name: (HERE / name).read_bytes()), mock.patch.object(bundle.subprocess, "check_output", return_value=self.tooling_sha + "\n"):
            with self.assertRaises(FileExistsError):
                bundle.generate(self.root, self.tooling_sha, self.profile_path, self.archive, self.root / "output")
        with mock.patch.object(bundle, "committed_file", return_value=b"dirty"), mock.patch.object(bundle.subprocess, "check_output", return_value=self.tooling_sha + "\n"):
            with self.assertRaisesRegex(ValueError, "executing tooling"):
                bundle.generate(self.root, self.tooling_sha, self.profile_path, self.archive, self.root / "new")
        self.archive.write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "archive checksum"):
            verify.verify_archive(self.archive, self.profile)

    def test_profile_schema_and_no_silent_certification(self):
        for field, value in (("native_certified", True), ("schema", 2), ("package_release", True), ("profile_id", "a';false"), ("architecture", "aarch64")):
            candidate = copy.deepcopy(self.profile)
            candidate[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                verify.validate_profile(candidate)
        candidate = copy.deepcopy(self.profile)
        candidate["hyprland"]["header_version"] = "0.57.0"
        with self.assertRaisesRegex(ValueError, "unchanged source"):
            verify.validate_profile(candidate)
        with self.assertRaisesRegex(ValueError, "duplicate JSON"):
            verify.read_json('{"schema":1,"schema":1}')

    def test_source_inventory_tamper_and_embedded_verifier_preserved(self):
        source = self.source()
        self.assertEqual(verify.verify_source(source, self.profile), self.manifest)
        for name in ("verify.py", "src/plugin.cpp", "SOURCE-PROVENANCE.json"):
            path = source / name
            original = path.read_bytes()
            path.write_bytes(b"tampered")
            with self.subTest(name=name), self.assertRaises(ValueError):
                verify.verify_source(source, self.profile)
            path.write_bytes(original)
        (source / "extra").write_text("extra")
        with self.assertRaisesRegex(ValueError, "inventory"):
            verify.verify_source(source, self.profile)
        (source / "extra").unlink()
        (source / "linked").symlink_to(source / "verify.py")
        with self.assertRaisesRegex(ValueError, "nonregular"):
            verify.verify_source(source, self.profile)

    def test_archive_refuses_links_traversal_duplicates_and_missing_files(self):
        for variant in ("symlink", "traversal", "duplicate", "missing"):
            raw = io.BytesIO()
            with tarfile.open(fileobj=raw, mode="w:gz") as contents:
                for name, data in self.files.items():
                    if variant == "missing" and name == "verify.py":
                        continue
                    info = tarfile.TarInfo(verify.STEM + "/" + name)
                    info.size = len(data)
                    contents.addfile(info, io.BytesIO(data))
                if variant != "missing":
                    info = tarfile.TarInfo(verify.STEM + "/" + {"symlink": "link", "traversal": "../escape", "duplicate": "verify.py"}[variant])
                    if variant == "symlink":
                        info.type, info.linkname = tarfile.SYMTYPE, "verify.py"
                    contents.addfile(info)
            candidate = self.root / (variant + ".tar.gz")
            candidate.write_bytes(raw.getvalue())
            profile = copy.deepcopy(self.profile)
            profile["source"]["archive_sha256"] = verify.digest(candidate)
            with self.subTest(variant=variant), self.assertRaises(ValueError):
                verify.verify_archive(candidate, profile)

    def test_kit_tampering_and_reviewed_digest_required(self):
        _, kit, _ = self.generate()
        expected = verify.digest(kit / "KIT-PROVENANCE.json")
        for name in ("PROFILE.json", "KIT-PROVENANCE.json", "profile_verify.py"):
            path = kit / name
            data = path.read_bytes()
            path.write_bytes(b"tamper")
            with self.subTest(name=name), self.assertRaises(ValueError):
                verify.verify_kit(kit, expected)
            path.write_bytes(data)
        with self.assertRaises(ValueError):
            verify.verify_kit(kit, "e" * 64)

    def test_recipe_tamper_refused_before_execution_and_tests_mandatory(self):
        _, kit, _ = self.generate()
        for name in (self.archive.name, "PROFILE.json", "KIT-PROVENANCE.json", "profile_verify.py"):
            path = kit / name
            data = path.read_bytes()
            path.write_bytes(b"raise SystemExit('must not execute')")
            result = subprocess.run(["bash", "-c", 'source "$1"; startdir="$2"; srcdir="$2"; SRCDEST="$2"; prepare', "test", str(kit / "PKGBUILD"), str(kit)], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("SyntaxError", result.stderr)
            path.write_bytes(data)
        script = '''source "$1"
_verify() { return 0; }
ctest() { [[ -z ${LD_PRELOAD+x} && -z ${FAKEROOTKEY+x} && -z ${FAKED_MODE+x} ]] || return 88; return 17; }
export LD_PRELOAD=fixture FAKEROOTKEY=fixture FAKED_MODE=fixture
package
result=$?
[[ $LD_PRELOAD == fixture && $FAKEROOTKEY == fixture && $FAKED_MODE == fixture ]] || exit 89
exit "$result"
'''
        result = subprocess.run(["bash", "-c", script, "test", str(kit / "PKGBUILD")], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)

    def test_profile_lifecycle_kit_preserves_source_identity(self):
        _, kit, metadata = self.generate()
        result = lifecycle.verify_profile_kit(kit, verify.SOURCE_REVISION, verify.DRIVER_VERSION, verify.digest(kit / "KIT-PROVENANCE.json"))
        self.assertEqual(result[0], self.manifest)
        self.assertEqual(result[2], self.profile)
        self.assertEqual(result[3], metadata)
        (kit / "build").mkdir()
        with self.assertRaisesRegex(ValueError, "fresh complete"):
            lifecycle.verify_profile_kit(kit, verify.SOURCE_REVISION, verify.DRIVER_VERSION, verify.digest(kit / "KIT-PROVENANCE.json"))

    def test_reviewed_recipe_reconstruction_refuses_changed_recipe_and_checksums(self):
        _, kit, _ = self.generate()
        recipe = kit / "PKGBUILD"
        original = verify.digest(recipe)
        recipe.write_bytes(recipe.read_bytes() + b"\n# unreviewed change\n")
        sums = kit / "SHA256SUMS"
        sums.write_text(sums.read_text().replace(original, verify.digest(recipe)))
        with self.assertRaisesRegex(ValueError, "recipe differs"):
            lifecycle.verify_profile_kit(kit, verify.SOURCE_REVISION, verify.DRIVER_VERSION, verify.digest(kit / "KIT-PROVENANCE.json"))

    def test_profile_package_payload_and_provenance(self):
        _, _, metadata = self.generate()
        module = b"synthetic module"
        runtime_sha = self.profile["runtime"]["sha256"]
        build = {"source": self.manifest, "profile": self.profile, "kit": metadata, "module_sha256": verify.sha256(module),
                 "module_runtime_sha256": runtime_sha, "compiler_runtime_sha256": runtime_sha, "compositor_runtime_sha256": runtime_sha}
        data = {lifecycle.MODULE: module, lifecycle.LICENSE: self.files["LICENSE.md"], lifecycle.SOURCE: self.files["SOURCE-PROVENANCE.json"],
                lifecycle.BUILD: verify.json_bytes(build), lifecycle.PROFILE: verify.json_bytes(self.profile),
                lifecycle.KIT: verify.json_bytes(metadata), lifecycle.VERIFIER: (HERE / "profile_verify.py").read_bytes()}
        names = list(data) + [".PKGINFO", ".BUILDINFO", ".MTREE"]
        info = "pkgname = cua-hyprland-plugin\npkgver = 0.24.0-2\narch = x86_64\ndepend = hyprland=0.56.2-2\ndepend = gcc-libs=16.2.1-1\ndepend = python>=3.11\ndepend = binutils\n"
        with mock.patch.object(lifecycle, "run", side_effect=lambda command: subprocess.CompletedProcess(command, 0, "\n".join(names) if "-tf" in command else info, "")), mock.patch.object(lifecycle.subprocess, "check_output", side_effect=lambda command: data[command[-1]]):
            self.assertEqual(lifecycle.package_payload(self.root / "package", self.manifest, self.profile, metadata), {name: verify.sha256(value) for name, value in data.items()})
            for name in (lifecycle.PROFILE, lifecycle.KIT, lifecycle.VERIFIER):
                original = data[name]
                data[name] = b"{}"
                with self.subTest(name=name), self.assertRaises(ValueError):
                    lifecycle.package_payload(self.root / "package", self.manifest, self.profile, metadata)
                data[name] = original
            names.append(".INSTALL")
            with self.assertRaisesRegex(ValueError, "payload or hooks"):
                lifecycle.package_payload(self.root / "package", self.manifest, self.profile, metadata)

    def test_profile_abi_dependencies_are_reviewed_safe_names(self):
        self.profile["runtime"]["packages"]["hyprutils"] = "0.14.2-1"
        verify.validate_profile(self.profile)
        self.profile["runtime"]["packages"]["bad';command"] = "1-1"
        with self.assertRaisesRegex(ValueError, "package name"):
            verify.validate_profile(self.profile)

    def test_exact_package_owned_header_tree_and_api_bytes(self):
        headers = self.root / "headers"
        api = headers / "src/plugins/PluginAPI.hpp"
        api.parent.mkdir(parents=True)
        api.write_text("synthetic API")
        with mock.patch.object(verify, "run", return_value=str(api) + "\n" + str(api.parent) + "/"):
            first = verify.header_inventory_sha256(headers)
            self.assertEqual(first, verify.sha256(verify.json_bytes({"src/plugins/PluginAPI.hpp": verify.digest(api)})))
            api.write_text("changed API")
            self.assertNotEqual(first, verify.header_inventory_sha256(headers))
            extra = headers / "extra.hpp"
            extra.write_text("unowned header")
            with self.assertRaisesRegex(ValueError, "package header inventory"):
                verify.header_inventory_sha256(headers)
            extra.unlink()
            api.unlink()
            api.symlink_to(self.archive)
            with self.assertRaisesRegex(ValueError, "nonregular"):
                verify.header_inventory_sha256(headers)

    def test_profile_lifecycle_uses_exact_dependency_and_package_revisions(self):
        self.profile["runtime"]["packages"]["hyprutils"] = "0.14.2-1"
        package = self.root / "candidate.pkg.tar.zst"
        package.write_text("package fixture")
        data = {lifecycle.MODULE: b"module fixture"}
        payload = {name: verify.sha256(value) for name, value in data.items()}
        installed = set()
        calls = []

        def fake_pacman(command, **kwargs):
            calls.append(command)
            root = Path(command[command.index("--root") + 1])
            operation = command[command.index("--noconfirm") + 1:]
            self.assertNotIn("--nodeps", command)
            self.assertEqual("--noscriptlet" in command, operation[0] in ("-U", "-R"))
            code, output = 0, ""
            if operation == ["-U", str(package)]:
                if root.name == "mismatched":
                    code, output = 1, "unable to satisfy dependency 'hyprland=0.56.2-2'"
                else:
                    for name, value in data.items():
                        path = root / name
                        path.parent.mkdir(parents=True, exist_ok=True)
                        path.write_bytes(value)
                    installed.add(root)
            elif operation[0] == "-R":
                for name in data:
                    (root / name).unlink()
                installed.remove(root)
            elif operation[0] == "-Q":
                code = 0 if root in installed else 1
                output = "cua-hyprland-plugin 0.24.0-2" if code == 0 else ""
            return subprocess.CompletedProcess(command, code, output, "")

        with mock.patch.object(lifecycle, "run", side_effect=fake_pacman):
            lifecycle.qualify(self.root, package, payload, self.manifest, self.profile)
        lifecycle.assert_state(self.root / "matching", payload, True)
        lifecycle.assert_state(self.root / "mismatched", payload, False)
        self.assertEqual(len(calls), 10)
        for name, version in (("hyprutils", "0.14.2-1"), ("gcc-libs", "16.2.1-1"), ("python", "3.11.0-1")):
            with tarfile.open(self.root / f"{name}-fixture.pkg.tar.gz") as archive:
                self.assertIn(f"pkgver = {version}\n".encode(), archive.extractfile(".PKGINFO").read())


class NativeProfileTest(unittest.TestCase):
    generate = ProfileTest.generate
    source = ProfileTest.source

    def setUp(self):
        ProfileTest.setUp(self)
        self.cxx = self.root / "g++"
        self.cxx.write_text("compiler")
        self.runtime = self.root / self.profile["runtime"]["basename"]
        self.runtime.write_text("runtime")
        self.profile["runtime"]["sha256"] = verify.digest(self.runtime)
        self.profile["compiler"]["sha256"] = verify.digest(self.cxx)
        self.calls = []
        self.overrides = {}
        self.probe_comment = None
        self.pkgconfig = {"executable": "/usr/bin/pkgconf", "executable_sha256": "e" * 64,
                          "pc_path": "/usr/share/pkgconfig/hyprland.pc", "pc_sha256": "f" * 64,
                          "query_environment": dict(verify.PKGCONF_ENV),
                          "cflags": ["-I/usr/include", "-I/usr/include/hyprland/protocols", "-I/usr/include/hyprland", "-I/usr/include/hyprland/src"],
                          "include_dirs": ["/usr/include", "/usr/include/hyprland/protocols", "/usr/include/hyprland", "/usr/include/hyprland/src"],
                          "cflags_other": [], "ldflags": ["-L/usr/lib", "-lhyprutils"]}

    def fake_run(self, *args, input=None):
        self.calls.append(args)
        if args in self.overrides:
            return self.overrides[args]
        if args[:2] == ("pacman", "-Q"):
            versions = {"hyprland": self.profile["hyprland"]["package_version"], **self.profile["runtime"]["packages"]}
            return args[2] + " " + versions[args[2]]
        if args[:2] == ("pacman", "-Qoq"):
            return "gcc-libs"
        if args[0] == str(verify.PKGCONF):
            return "0.56.2"
        if args[:3] == ("readelf", "-p", ".comment"):
            if args[-1].endswith("probe.o") and self.probe_comment:
                return self.probe_comment
            return "  [ 0]  " + self.profile["compiler"]["comment"]
        if args[:2] == ("readelf", "-d"):
            return "Shared library: [libstdc++.so.6]"
        if args[0] == "ldd":
            return f" libstdc++.so.6 => {self.runtime} (0x0)"
        if "-dM" in args:
            return '#define __VERSION__ "' + self.profile["compiler"]["version"] + '"'
        if "--print-file-name=libstdc++.so.6" in args:
            return str(self.runtime)
        return ""

    def native_context(self):
        def fake_selection(profile):
            verify.require(verify.run(str(verify.PKGCONF), "--modversion", "hyprland") == profile["hyprland"]["header_version"], "Hyprland header mismatch")
            return self.pkgconfig
        patches = [mock.patch.object(verify.platform, "system", return_value="Linux"),
                   mock.patch.object(verify.platform, "machine", return_value="x86_64"),
                   mock.patch.object(verify, "run", side_effect=self.fake_run),
                   mock.patch.object(verify, "header_inventory_sha256", return_value="d" * 64),
                   mock.patch.object(verify, "pkgconfig_selection", side_effect=fake_selection),
                   mock.patch.dict(verify.os.environ, {}, clear=True)]
        real_digest = verify.digest
        patches.append(mock.patch.object(verify, "digest", side_effect=lambda path: self.profile["hyprland"]["sha256"] if str(path) == "/usr/bin/Hyprland" else real_digest(path)))
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)

    def test_native_profile_exact_checks_and_refusals(self):
        self.native_context()
        native = verify.verify_native(self.cxx, self.profile)
        self.assertEqual(native["compiler_runtime_sha256"], self.profile["runtime"]["sha256"])
        for command, output in ((('pacman', '-Q', 'hyprland'), 'hyprland 0.56.2-1'),
                                (('pacman', '-Q', 'gcc-libs'), 'gcc-libs 0-1'),
                                (('/usr/bin/pkgconf', '--modversion', 'hyprland'), '0.56.3'),
                                ((str(self.cxx), '-dM', '-E', '-x', 'c++', '-'), '#define __VERSION__ "wrong"'),
                                (('readelf', '-p', '.comment', '/usr/bin/Hyprland'), 'wrong comment'),
                                (('readelf', '-d', '/usr/bin/Hyprland'), 'static runtime')):
            self.overrides[command] = output
            with self.subTest(command=command), self.assertRaises(ValueError):
                verify.verify_native(self.cxx, self.profile)
            self.overrides.clear()
        with mock.patch.object(verify, "header_inventory_sha256", return_value="e" * 64), self.assertRaisesRegex(ValueError, "header inventory"):
            verify.verify_native(self.cxx, self.profile)
        self.probe_comment = "emitted by a different compiler"
        with self.assertRaisesRegex(ValueError, "ELF compiler comment"):
            verify.verify_native(self.cxx, self.profile)
        self.probe_comment = None
        self.overrides[("pacman", "-Qoq", str(self.runtime.resolve()))] = "unreviewed-runtime-owner"
        with self.assertRaisesRegex(ValueError, "owner is not pinned"):
            verify.verify_native(self.cxx, self.profile)
        self.overrides.clear()
        self.cxx.write_text("different compiler executable")
        with self.assertRaisesRegex(ValueError, "compiler checksum"):
            verify.verify_native(self.cxx, self.profile)
        self.cxx.write_text("compiler")
        self.runtime.write_text("different bytes")
        with self.assertRaisesRegex(ValueError, "runtime mismatch"):
            verify.verify_native(self.cxx, self.profile)

    def test_missing_or_malformed_non_cpp_dependency_refuses(self):
        self.native_context()
        valid = f" libstdc++.so.6 => {self.runtime} (0x0)"
        for other in ("libhyprutils.so.13 => not found", "unexpected loader diagnostic",
                      "libc.so.6 => relative/path (0x0)"):
            self.overrides[("ldd", "/usr/bin/Hyprland")] = valid + "\n " + other
            with self.subTest(other=other), self.assertRaisesRegex(ValueError, "shared dependency"):
                verify.verify_environment(self.profile)
        self.overrides[("ldd", "/usr/bin/Hyprland")] = valid
        self.overrides[("readelf", "-d", "/usr/bin/Hyprland")] = \
            "Shared library: [libstdc++.so.6]\nShared library: [libhyprutils.so.13]"
        with self.assertRaisesRegex(ValueError, "missing shared dependency resolution"):
            verify.verify_environment(self.profile)
        self.overrides.clear()
        self.overrides[("ldd", "/usr/bin/Hyprland")] = \
            "linux-vdso.so.1 (0x7fff)\n" + valid + "\n /lib64/ld-linux-x86-64.so.2 (0x7ff0)"
        verify.verify_environment(self.profile)

    def test_build_configuration_and_shared_runtime_checks(self):
        self.native_context()
        source = self.source()
        build = self.root / "build"
        build.mkdir()
        (build / "cua-hyprland-plugin.so").write_text("module")
        expected = dict(verify.OPTIONS, BUILD_TESTING="ON", CUA_HYPRLAND_BUILD_PLUGIN="ON", CMAKE_BUILD_TYPE="Release",
                        CMAKE_GENERATOR="Ninja",
                        PKG_CONFIG_EXECUTABLE="/usr/bin/pkgconf", PKG_CONFIG_ARGN="", PKG_CONFIG_USE_CMAKE_PREFIX_PATH="OFF",
                        HYPRLAND_VERSION="0.56.2", HYPRLAND_CFLAGS=";".join(self.pkgconfig["cflags"]),
                        HYPRLAND_INCLUDE_DIRS=";".join(self.pkgconfig["include_dirs"]), HYPRLAND_CFLAGS_OTHER=";".join(self.pkgconfig["cflags_other"]),
                        HYPRLAND_LDFLAGS=";".join(self.pkgconfig["ldflags"]),
                        CUA_HYPRLAND_EXPECTED_VERSION="0.56.2", CUA_HYPRLAND_TEST_OPERATOR_KEY="",
                        CMAKE_CXX_COMPILER=str(self.cxx), CMAKE_HOME_DIRECTORY=str(source.resolve()))
        cache = build / "CMakeCache.txt"
        cache.write_text("".join(f"{key}:STRING={value}\n" for key, value in expected.items()))
        verify.verify_build(build, source, self.cxx, self.profile)
        for key in expected:
            changed = dict(expected, **{key: "WRONG"})
            cache.write_text("".join(f"{name}:STRING={value}\n" for name, value in changed.items()))
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "build configuration mismatch"):
                verify.verify_build(build, source, self.cxx, self.profile)
        for name, value in (("CMAKE_CXX_FLAGS", "-O2 -I/alternate-same-version"),
                            ("CMAKE_TOOLCHAIN_FILE", "/alternate/toolchain.cmake"),
                            ("CMAKE_CXX_COMPILER_LAUNCHER", "/alternate/launcher")):
            changed = dict(expected, **{name: value})
            cache.write_text("".join(f"{key}:STRING={setting}\n" for key, setting in changed.items()))
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "override refused"):
                verify.verify_build(build, source, self.cxx, self.profile)

    def test_header_and_toolchain_environment_overrides_refused(self):
        for name in ("PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR", "PKG_CONFIG_SYSROOT_DIR", "PKG_CONFIG", "PKGCONF_PKG_PKGF",
                     "PKG_CONFIG_ALLOW_SYSTEM_CFLAGS", "PKG_CONFIG_ALLOW_SYSTEM_LIBS",
                     "CPATH", "CPLUS_INCLUDE_PATH", "GCC_EXEC_PREFIX", "COMPILER_PATH", "LIBRARY_PATH",
                     "CMAKE_PREFIX_PATH", "CMAKE_TOOLCHAIN_FILE"):
            with self.subTest(name=name), mock.patch.dict(verify.os.environ, {name: "/alternate-same-version"}, clear=True), self.assertRaisesRegex(ValueError, "routing environment refused"):
                verify.verify_build_environment()
        for flags in ("-O2 -I/alternate", "-isystem /alternate", "-include /alternate/header.h", "@/alternate/flags",
                      "-Wp,-I/alternate", "-B/alternate", "--sysroot=/alternate", "-specs=/alternate/specs",
                      "-Wl,-rpath,/alternate"):
            with self.subTest(flags=flags), mock.patch.dict(verify.os.environ, {"CXXFLAGS": flags}, clear=True), self.assertRaisesRegex(ValueError, "flag override refused"):
                verify.verify_build_environment()
        with mock.patch.dict(verify.os.environ, {"CXXFLAGS": "-march=x86-64 -O2 -pipe -Wp,-D_FORTIFY_SOURCE=3 -fstack-protector-strong",
                                               "LDFLAGS": "-Wl,-O1,--sort-common,--as-needed,-z,relro,-z,now"}, clear=True):
            verify.verify_build_environment()

    def test_canonical_pkgconf_rejects_alternate_same_version_headers(self):
        root = self.root.resolve()
        system = root / "usr/include"
        headers = system / "hyprland"
        alternate = system / "alternate-same-version"
        headers.mkdir(parents=True)
        alternate.mkdir()
        pc = root / "usr/share/pkgconfig/hyprland.pc"
        pc.parent.mkdir(parents=True)
        pc.write_text("canonical metadata fixture")
        executable = root / "usr/bin/pkgconf"
        executable.parent.mkdir(parents=True)
        executable.write_text("pkgconf fixture")
        include_flags = f"-I{headers}"
        pc_directory = str(pc.parent)

        def fake_run(*args, **kwargs):
            if args[:2] == ("pacman", "-Qoq"):
                return "pkgconf" if args[2] == str(executable) else "hyprland"
            self.assertEqual(kwargs.get("extra_env"), verify.PKGCONF_ENV)
            return {"--variable=pcfiledir": pc_directory, "--modversion": "0.56.2",
                    "--cflags": include_flags + " -pthread", "--cflags-only-I": include_flags,
                    "--cflags-only-other": "-pthread", "--libs": "-L/usr/lib -lhyprutils"}[args[1]]

        with mock.patch.object(verify, "PKGCONF", executable), mock.patch.object(verify, "HYPRLAND_PC", pc), \
                mock.patch.object(verify, "HEADER_ROOT", headers), mock.patch.object(verify, "SYSTEM_INCLUDE", system), \
                mock.patch.object(verify, "run", side_effect=fake_run):
            selected = verify.pkgconfig_selection(self.profile)
            self.assertEqual(selected["pc_sha256"], verify.digest(pc))
            self.assertEqual(selected["include_dirs"], [str(headers)])
            self.assertEqual(selected["query_environment"], verify.PKGCONF_ENV)
            self.assertEqual(selected["ldflags"], ["-L/usr/lib", "-lhyprutils"])
            (headers / "protocols").mkdir()
            (headers / "src").mkdir()
            include_flags = f"-I{headers}/protocols -I{headers} -I{headers}/src"
            self.assertEqual(verify.pkgconfig_selection(self.profile)["include_dirs"],
                             [str(headers / "protocols"), str(headers), str(headers / "src")])
            include_flags = f"-I{system} -I{headers}/protocols -I{headers} -I{headers}/src"
            self.assertEqual(verify.pkgconfig_selection(self.profile)["include_dirs"],
                             [str(system), str(headers / "protocols"), str(headers), str(headers / "src")])
            (system / "src").mkdir()
            with self.assertRaisesRegex(ValueError, "unreviewed system src"):
                verify.pkgconfig_selection(self.profile)
            (system / "src").rmdir()
            include_flags = f"-I{headers} -I{system}"
            with self.assertRaisesRegex(ValueError, "only as the leading entry"):
                verify.pkgconfig_selection(self.profile)
            include_flags = f"-I{system} -I{alternate} -I{headers}"
            with self.assertRaisesRegex(ValueError, "not first"):
                verify.pkgconfig_selection(self.profile)
            include_flags = f"-I{alternate} -I{headers}"
            with self.assertRaisesRegex(ValueError, "not first"):
                verify.pkgconfig_selection(self.profile)
            include_flags = f"-I{headers}"
            pc_directory = str(root / "alternate-same-version/pkgconfig")
            with self.assertRaisesRegex(ValueError, "noncanonical Hyprland pkg-config source"):
                verify.pkgconfig_selection(self.profile)

    def test_pkgconf_fixed_query_environment_does_not_mutate_caller(self):
        with mock.patch.dict(verify.os.environ, {"LC_ALL": "caller-locale"}, clear=True), \
                mock.patch.object(verify.subprocess, "check_output", return_value="flags\n") as execute:
            self.assertEqual(verify.run("/usr/bin/pkgconf", "--cflags", "hyprland", extra_env=verify.PKGCONF_ENV), "flags")
            self.assertEqual(execute.call_args.kwargs["env"], dict(verify.PKGCONF_ENV, LC_ALL="C"))
            self.assertEqual(dict(verify.os.environ), {"LC_ALL": "caller-locale"})

    def test_consumer_does_not_invoke_compiler_or_headers_and_refuses_drift(self):
        self.native_context()
        self.profile_path.write_bytes(verify.json_bytes(self.profile))
        _, kit, metadata = self.generate()
        module = self.root / "module.so"
        module.write_text("module")
        native = verify.verify_native(self.cxx, self.profile)
        build = dict(native, source=self.manifest, profile=self.profile, kit=metadata,
                     module_sha256=verify.digest(module), module_runtime_sha256=self.profile["runtime"]["sha256"])
        (kit / "BUILD-PROVENANCE.json").write_bytes(verify.json_bytes(build))
        self.cxx.unlink()
        self.calls.clear()
        verify.verify_consumer(module, kit, self.profile, metadata)
        self.assertTrue(all(call[0] in ("pacman", "readelf", "ldd") for call in self.calls))
        module.write_text("drift")
        with self.assertRaisesRegex(ValueError, "module checksum"):
            verify.verify_consumer(module, kit, self.profile, metadata)


if __name__ == "__main__":
    unittest.main()
