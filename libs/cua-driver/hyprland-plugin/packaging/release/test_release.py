"""Source-release tests; native commands use synthetic contract fixtures."""

import importlib.util
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock


HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


bundle = load("bundle")
verify = load("verify")


class ReleaseTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cua-source-release-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-q")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "user.name", "Release Fixture")
        # Synthetic source keeps this suite independent of uncommitted product work.
        for name in bundle.SOURCE_FILES:
            self.write(bundle.PLUGIN + name, "// synthetic public source\n")
        self.write(bundle.PLUGIN + "CMakeLists.txt", "project(cua_hyprland_plugin VERSION 0.1.0 LANGUAGES CXX)\n")
        self.write("libs/cua-driver/rust/Cargo.toml", '[workspace.package]\nversion = "1.2.3"\n')
        self.write("LICENSE.md", "Synthetic fixture license\n")
        for name in ("verify.py", "PKGBUILD.in", "USAGE.md"):
            self.write(bundle.RELEASE + name, (HERE / name).read_text())
        self.write("private/secret.txt", "excluded sentinel\n")
        self.write(bundle.PLUGIN + "tests/evidence/trace.json", "excluded sentinel\n")
        self.write(bundle.PLUGIN + "src/private.hpp", "excluded sentinel\n")
        self.git("add", ".")
        self.git("commit", "-qm", "Synthetic release fixture")
        self.revision = self.git("rev-parse", "HEAD")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], stderr=subprocess.PIPE, text=True).strip()

    def write(self, path, data):
        destination = self.repo / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(data)

    def generate(self, name="release"):
        output = self.root / name
        bundle.generate(self.repo, self.revision, "1.2.3", output)
        return output

    def source(self):
        output = self.generate()
        tarball = next(output.glob("*.tar.gz"))
        destination = self.root / "extracted"
        with tarfile.open(tarball) as archive:
            archive.extractall(destination, filter="data")
        return next(destination.iterdir())

    def test_deterministic_and_allowlisted_committed_source(self):
        first = self.generate("first")
        self.write(bundle.PLUGIN + "src/plugin.cpp", "dirty source must not ship\n")
        self.write(bundle.PLUGIN + "src/untracked.hpp", "untracked must not ship\n")
        self.write(bundle.RELEASE + "PKGBUILD.in", "dirty recipe must not ship\n")
        second = self.generate("second")
        self.assertEqual({p.name: p.read_bytes() for p in first.iterdir()}, {p.name: p.read_bytes() for p in second.iterdir()})
        with tarfile.open(next(first.glob("*.tar.gz"))) as archive:
            names = {"/".join(member.name.split("/")[1:]) for member in archive.getmembers()}
            self.assertEqual(names, set(bundle.SOURCE_FILES) | {"LICENSE.md", "verify.py", "SOURCE-PROVENANCE.json"})
            for member in archive.getmembers():
                self.assertTrue(member.isfile())
                self.assertEqual((member.uid, member.gid, member.mtime, member.mode), (0, 0, 0, 0o644))
                self.assertNotIn(b"excluded sentinel", archive.extractfile(member).read())
        self.assertEqual((first / "README.md").read_bytes(), (HERE / "USAGE.md").read_bytes())
        subprocess.run(["bash", "-n", str(first / "PKGBUILD")], check=True)

    def test_refuses_revision_version_missing_source_and_existing_output(self):
        for revision, version in (("HEAD", "1.2.3"), (self.revision, "1.2.4"), (self.revision, "1.2.3;false")):
            with self.assertRaises(ValueError):
                bundle.generate(self.repo, revision, version, self.root / "invalid")
        output = self.generate()
        with self.assertRaises(FileExistsError):
            bundle.generate(self.repo, self.revision, "1.2.3", output)
        self.git("rm", bundle.PLUGIN + "src/plugin.cpp")
        self.git("commit", "-qm", "Missing required source")
        with self.assertRaisesRegex(ValueError, "required committed regular file"):
            bundle.generate(self.repo, self.git("rev-parse", "HEAD"), "1.2.3", self.root / "missing")

    def test_source_tampering_and_provenance_mismatch(self):
        source = self.source()
        manifest = verify.verify_source(source, self.revision, "1.2.3")
        self.assertFalse(manifest["native_certified"])
        for revision, version in (("0" * 40, "1.2.3"), (self.revision, "1.2.4")):
            with self.assertRaisesRegex(ValueError, "provenance mismatch"):
                verify.verify_source(source, revision, version)
        original = (source / "src/plugin.cpp").read_bytes()
        (source / "src/plugin.cpp").write_text("tampered\n")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            verify.verify_source(source, self.revision, "1.2.3")
        (source / "src/plugin.cpp").write_bytes(original)
        (source / "extra.txt").write_text("unexpected\n")
        with self.assertRaisesRegex(ValueError, "inventory mismatch"):
            verify.verify_source(source, self.revision, "1.2.3")

    def test_symlink_source_refused(self):
        self.git("rm", bundle.PLUGIN + "src/plugin.cpp")
        (self.repo / bundle.PLUGIN / "src/plugin.cpp").symlink_to("../CMakeLists.txt")
        self.git("add", bundle.PLUGIN + "src/plugin.cpp")
        self.git("commit", "-qm", "Symlink source fixture")
        with self.assertRaisesRegex(ValueError, "required committed regular file"):
            bundle.generate(self.repo, self.git("rev-parse", "HEAD"), "1.2.3", self.root / "symlink")

    def test_archive_tampering_refused_before_verifier_execution(self):
        output = self.generate()
        tarball = next(output.glob("*.tar.gz"))
        tarball.write_bytes(b"tampered archive")
        result = subprocess.run(["bash", "-c", 'source "$1"; SRCDEST="$2"; srcdir="$2"; prepare', "test", str(output / "PKGBUILD"), str(output)], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAILED", result.stdout)
        self.assertNotIn("verify.py", result.stderr)

    def test_recipe_installs_only_module_license_and_provenance(self):
        output = self.generate()
        srcdir = self.root / "src"
        build = srcdir / "build"
        build.mkdir(parents=True)
        (build / "cua-hyprland-plugin.so").write_text("module fixture")
        stem = next(output.glob("*.tar.gz")).name.removesuffix(".tar.gz")
        source = srcdir / stem
        source.mkdir()
        (source / "LICENSE.md").write_text("license fixture")
        (source / "SOURCE-PROVENANCE.json").write_text("{}")
        (srcdir / "BUILD-PROVENANCE.json").write_text("{}")
        pkgdir = self.root / "pkg"
        script = '''source "$1"
srcdir="$2"; pkgdir="$3"
check() { :; }
_verify() { :; }
if command -v ginstall >/dev/null; then
  install() { command ginstall "$@"; }
fi
package || exit 1
[[ -z ${install:-} ]]
'''
        subprocess.run(["bash", "-c", script, "test", str(output / "PKGBUILD"), str(srcdir), str(pkgdir)], check=True)
        self.assertEqual({str(p.relative_to(pkgdir)) for p in pkgdir.rglob("*") if p.is_file()}, {
            "usr/lib/cua/hyprland/cua-hyprland-plugin.so",
            "usr/share/licenses/cua-hyprland-plugin/LICENSE",
            "usr/share/cua-hyprland-plugin/SOURCE-PROVENANCE.json",
            "usr/share/cua-hyprland-plugin/BUILD-PROVENANCE.json",
        })
        text = (output / "PKGBUILD").read_text()
        for forbidden in ("CUA_SOURCE_ROOT", "hyprctl", "systemctl", "hyprland.conf", "SKIP", "post_install", "post_upgrade"):
            self.assertNotIn(forbidden, text)
        failed = subprocess.run(["bash", "-c", 'source "$1"; check() { return 17; }; package', "test", str(output / "PKGBUILD")])
        self.assertNotEqual(failed.returncode, 0)

    def test_manifest_and_verifier_tampering_refused_before_execution(self):
        source = self.source()
        output = self.root / "release"
        script = 'source "$1"; SRCDEST="$2"; srcdir="$3"; prepare'
        for path in (source / "SOURCE-PROVENANCE.json", source / "verify.py"):
            with self.subTest(path=path.name):
                original = path.read_bytes()
                path.write_text("tampered content\n")
                result = subprocess.run(["bash", "-c", script, "test", str(output / "PKGBUILD"), str(output), str(source.parent)], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("FAILED", result.stdout)
                self.assertNotIn("SyntaxError", result.stderr)
                path.write_bytes(original)


class NativeContractTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cua-release-native-fixture-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.cxx = self.root / "g++"
        self.cxx.write_text("compiler fixture")
        self.runtime = self.root / "libstdc++.so.6.0.36"
        self.runtime.write_text("runtime fixture")
        self.package = "hyprland 0.56.2-1"
        self.headers = "0.56.2"
        self.compiler = verify.COMPILER_VERSION
        self.comment = verify.COMPILER_COMMENT
        self.probe_comment = verify.COMPILER_COMMENT

    def fake_run(self, *args, input=None):
        if args[:2] == ("pacman", "-Q"):
            return self.package
        if args[0] == "pkg-config":
            return self.headers
        if args[0] == "readelf":
            comment = self.comment if args[-1] == "/usr/bin/Hyprland" else self.probe_comment
            return f"String dump of section '.comment':\n  [ 0]  {comment}"
        if args[0] == "ldd":
            return f" libstdc++.so.6 => {self.runtime} (0x0000)"
        if "-dM" in args:
            return f'#define __VERSION__ "{self.compiler}"'
        if "--print-file-name=libstdc++.so.6" in args:
            return str(self.runtime)
        return "synthetic compiler"

    def native(self):
        with mock.patch.object(verify.platform, "system", return_value="Linux"), mock.patch.object(verify.platform, "machine", return_value="x86_64"), mock.patch.object(verify, "run", side_effect=self.fake_run), mock.patch.object(verify, "digest", return_value="fixture-sha256"):
            return verify.verify_native(self.cxx)

    def test_exact_native_contract_and_mismatch_refusals(self):
        self.assertEqual(self.native()["compiler_probe_comment"], verify.COMPILER_COMMENT)
        for field, bad in (("package", "hyprland 0.56.2-2"), ("headers", "0.56.3"), ("compiler", "16.1.1 20260729"), ("comment", "GCC: (GNU) 16.1.1 20260729"), ("probe_comment", "GCC: (GNU) 16.1.1 20260729")):
            with self.subTest(field=field):
                original = getattr(self, field)
                setattr(self, field, bad)
                with self.assertRaises(ValueError):
                    self.native()
                setattr(self, field, original)
        self.runtime = self.root / "libstdc++.so.6.0.35"
        self.runtime.write_text("old runtime")
        with self.assertRaisesRegex(ValueError, "runtime mismatch"):
            self.native()

    def test_build_option_mismatch_refused(self):
        build = self.root / "build"
        build.mkdir()
        (build / "CMakeCache.txt").write_text("CUA_HYPRLAND_INPUT:BOOL=OFF\n")
        with self.assertRaisesRegex(ValueError, "build configuration mismatch: CUA_HYPRLAND_INPUT"):
            verify.verify_build(build, self.root, self.cxx, "fixture-sha256")

    def test_module_runtime_mismatch_refused(self):
        build = self.root / "build"
        build.mkdir()
        cache = dict(verify.OPTIONS, BUILD_TESTING="ON", CUA_HYPRLAND_BUILD_PLUGIN="ON",
                     CUA_HYPRLAND_EXPECTED_VERSION="0.56.2", CMAKE_BUILD_TYPE="Release",
                     CUA_HYPRLAND_TEST_OPERATOR_KEY="", CMAKE_CXX_COMPILER=str(self.cxx),
                     CMAKE_HOME_DIRECTORY=str(self.root.resolve()))
        (build / "CMakeCache.txt").write_text("".join(f"{key}:STRING={value}\n" for key, value in cache.items()))
        with mock.patch.object(verify, "run", return_value="Shared library: [libstdc++.so.6]"), mock.patch.object(verify, "linked_runtime", return_value="different-runtime"):
            with self.assertRaisesRegex(ValueError, "module and compiler shared runtimes differ"):
                verify.verify_build(build, self.root, self.cxx, "fixture-sha256")


if __name__ == "__main__":
    unittest.main()
