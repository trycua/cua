"""Source-release tests; native commands use synthetic contract fixtures."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import textwrap
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
        for name in ("verify.py", "PKGBUILD.in", "USAGE.md", "lifecycle.py"):
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

    def test_release_archives_match_local_kit_and_are_deterministic(self):
        self.git("tag", "-a", "cua-driver-rs-v1.2.3", "-m", "Synthetic component tag")
        local = self.generate()
        outputs = [self.root / name for name in ("assets-first", "assets-second")]
        for output in outputs:
            bundle.generate(self.repo, self.revision, "1.2.3", output, release_assets=True)
            self.write(bundle.RELEASE + "USAGE.md", "dirty documentation must not ship")
        self.assertEqual({p.name: p.read_bytes() for p in outputs[0].iterdir()},
                         {p.name: p.read_bytes() for p in outputs[1].iterdir()})
        stem = f"cua-hyprland-plugin-1.2.3-{self.revision}"
        self.assertEqual({p.name for p in outputs[0].iterdir()},
                         {f"{stem}.tar.gz", f"{stem}-build-kit.tar.gz"})
        self.assertEqual((outputs[0] / f"{stem}.tar.gz").read_bytes(),
                         (local / f"{stem}.tar.gz").read_bytes())
        with tarfile.open(outputs[0] / f"{stem}-build-kit.tar.gz") as archive:
            self.assertEqual(archive.getnames(), sorted([
                "PKGBUILD", "README.md", "SHA256SUMS", "SOURCE-PROVENANCE.json", "lifecycle.py",
            ]))
            kit = {member.name: archive.extractfile(member).read() for member in archive.getmembers()}
            for member in archive.getmembers():
                self.assertTrue(member.isfile())
                self.assertEqual((member.uid, member.gid, member.mtime, member.mode), (0, 0, 0, 0o644))
                self.assertEqual(kit[member.name], (local / member.name).read_bytes())
        for line in kit["SHA256SUMS"].decode().splitlines():
            digest, name = line.split("  ")
            self.assertEqual(digest, bundle.sha256((local / name).read_bytes()))
        self.assertFalse(json.loads(kit["SOURCE-PROVENANCE.json"])["native_certified"])
        with self.assertRaises(FileExistsError):
            bundle.generate(self.repo, self.revision, "1.2.3", outputs[0], release_assets=True)

    def test_release_assets_require_exact_tag_and_committed_version(self):
        output = self.root / "invalid-assets"
        with self.assertRaises(subprocess.CalledProcessError):
            bundle.generate(self.repo, self.revision, "1.2.3", output, release_assets=True)
        self.git("tag", "cua-driver-rs-v1.2.3")
        self.write("unrelated.txt", "another commit")
        self.git("add", "unrelated.txt")
        self.git("commit", "-qm", "Different source revision")
        with self.assertRaisesRegex(ValueError, "exact Driver release tag"):
            bundle.generate(self.repo, self.git("rev-parse", "HEAD"), "1.2.3", output, release_assets=True)
        self.git("tag", "cua-driver-rs-v1.2.4", self.revision)
        for version in ("1.2.4", "1.2.3-nightly.20260905"):
            with self.assertRaises(ValueError):
                bundle.generate(self.repo, self.revision, version, output, release_assets=True)
        self.assertFalse(output.exists())

    def workflow_generate(self, version="1.2.3"):
        workflow = (HERE.parents[4] / ".github/workflows/cd-rust-cua-driver.yml").read_text()
        job = workflow.split("  build-hyprland-plugin-source:\n", 1)[1]
        script = textwrap.dedent(job.split("        run: |\n", 1)[1].split(
            "      - uses: actions/upload-artifact@v4", 1
        )[0])
        binaries = self.root / "bin"
        binaries.mkdir(exist_ok=True)
        python = binaries / "python3"
        if not python.exists():
            python.symlink_to(sys.executable)
        return subprocess.run(["bash", "-e", "-c", script], cwd=self.repo,
                              env={**os.environ, "PATH": f"{binaries}:{os.environ['PATH']}",
                                   "GITHUB_REF": "refs/heads/main", "REQUESTED_VERSION": version,
                                   "GITHUB_OUTPUT": str(self.root / "step-output")},
                              capture_output=True, text=True)

    def test_workflow_legacy_recovery_still_checks_source_and_version(self):
        self.git("tag", "cua-driver-rs-v1.2.3")
        legacy = self.workflow_generate()
        self.assertEqual(legacy.returncode, 0, legacy.stderr)
        self.assertIn("Legacy source has no plugin bundler", legacy.stdout)
        self.assertFalse((self.root / "step-output").exists())
        self.git("tag", "cua-driver-rs-v1.2.4")
        self.assertNotEqual(self.workflow_generate("1.2.4").returncode, 0)
        self.write("unrelated.txt", "different source")
        self.git("add", "unrelated.txt")
        self.git("commit", "-qm", "Different checkout")
        self.assertNotEqual(self.workflow_generate().returncode, 0)

    def test_workflow_existing_bundler_failure_is_not_a_legacy_skip(self):
        self.write(bundle.RELEASE + "bundle.py", "raise SystemExit(17)\n")
        self.git("add", bundle.RELEASE + "bundle.py")
        self.git("commit", "-qm", "Existing incompatible bundler")
        self.git("tag", "cua-driver-rs-v1.2.3")
        result = self.workflow_generate()
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertNotIn("Legacy source", result.stdout)
        self.assertFalse((self.root / "step-output").exists())

    def test_workflow_generates_both_assets_from_the_tag(self):
        self.write(bundle.RELEASE + "bundle.py", (HERE / "bundle.py").read_text())
        self.git("add", bundle.RELEASE + "bundle.py")
        self.git("commit", "-qm", "Release-capable bundler")
        self.git("tag", "cua-driver-rs-v1.2.3")
        result = self.workflow_generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "step-output").read_text(), "available=true\n")
        stem = f"cua-hyprland-plugin-1.2.3-{self.git('rev-parse', 'HEAD')}"
        self.assertEqual({p.name for p in (self.repo / "plugin-release-assets").iterdir()},
                         {f"{stem}.tar.gz", f"{stem}-build-kit.tar.gz"})

    def prepare_staged_assets(self):
        self.write(bundle.RELEASE + "bundle.py", (HERE / "bundle.py").read_text())
        self.git("add", bundle.RELEASE + "bundle.py")
        self.git("commit", "-qm", "Release-capable bundler")
        self.revision = self.git("rev-parse", "HEAD")
        self.git("tag", "cua-driver-rs-v1.2.3")
        output = self.repo / "release-upload"
        bundle.generate(self.repo, self.revision, "1.2.3", output, release_assets=True)
        return sorted(output.iterdir())

    def workflow_verify_staged_assets(self):
        workflow = (HERE.parents[4] / ".github/workflows/cd-rust-cua-driver.yml").read_text()
        step = workflow.split("      - name: Verify staged plugin source assets\n", 1)[1]
        script = textwrap.dedent(step.split("        run: |\n", 1)[1].split("      - name:", 1)[0])
        script = script.replace("${{ steps.version.outputs.version }}", "1.2.3").replace(
            "${{ steps.version.outputs.sha }}", self.revision)
        binaries = self.root / "bin"
        binaries.mkdir(exist_ok=True)
        (binaries / "python3").symlink_to(sys.executable)
        return subprocess.run(["bash", "-e", "-c", script], cwd=self.repo,
                              env={**os.environ, "PATH": f"{binaries}:{os.environ['PATH']}"},
                              capture_output=True, text=True)

    def test_staged_plugin_assets_match_exact_tag_bytes(self):
        self.prepare_staged_assets()
        result = self.workflow_verify_staged_assets()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_staged_plugin_assets_refuse_missing_archive(self):
        assets = self.prepare_staged_assets()
        assets[0].rename(self.root / "withheld.tar.gz")
        self.assertNotEqual(self.workflow_verify_staged_assets().returncode, 0)

    def test_staged_plugin_assets_refuse_extra_archive(self):
        self.prepare_staged_assets()
        self.write("release-upload/cua-hyprland-plugin-unexpected.tar.gz", "unexpected")
        self.assertNotEqual(self.workflow_verify_staged_assets().returncode, 0)

    def test_staged_plugin_assets_refuse_wrong_revision_filename(self):
        assets = self.prepare_staged_assets()
        assets[0].rename(assets[0].with_name(assets[0].name.replace(self.revision, "0" * 40)))
        self.assertNotEqual(self.workflow_verify_staged_assets().returncode, 0)

    def test_staged_plugin_assets_refuse_changed_bytes(self):
        assets = self.prepare_staged_assets()
        assets[0].write_bytes(b"altered download")
        self.assertNotEqual(self.workflow_verify_staged_assets().returncode, 0)

    def test_staged_legacy_assets_need_no_plugin(self):
        result = self.workflow_verify_staged_assets()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_staged_legacy_assets_refuse_unexpected_plugin(self):
        self.write("release-upload/cua-hyprland-plugin-unexpected.tar.gz", "unexpected")
        self.assertNotEqual(self.workflow_verify_staged_assets().returncode, 0)

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

    def test_mandatory_tests_drop_fakeroot_only_in_test_child(self):
        output = self.generate()
        binaries = self.root / 'test-bin'
        binaries.mkdir()
        ctest = binaries / 'ctest'
        ctest.write_text('''#!/bin/sh
test -z "${LD_PRELOAD+x}" && test -z "${FAKEROOTKEY+x}" && test -z "${FAKED_MODE+x}" || exit 91
test "$1" = --test-dir && test "$3" = --output-on-failure && test "$4" = --no-tests=error || exit 92
printf 'ctest-real-identity\\n'
exit "${TEST_CTEST_EXIT:-0}"
''')
        ctest.chmod(0o755)
        script = '''source "$1"
srcdir="$2"; PATH="$3:$PATH"
export LD_PRELOAD=synthetic-fakeroot.so FAKEROOTKEY=123 FAKED_MODE=unknown-is-real
_verify() { [[ "$LD_PRELOAD" == synthetic-fakeroot.so && "$FAKEROOTKEY" == 123 ]]; }
if [[ "$4" == check ]]; then
  check; result=$?
  [[ $result == 0 ]] || exit 93
else
  export TEST_CTEST_EXIT=17
  package; result=$?
  [[ $result != 0 ]] || exit 94
fi
[[ "$LD_PRELOAD" == synthetic-fakeroot.so && "$FAKEROOTKEY" == 123 && "$FAKED_MODE" == unknown-is-real ]]
'''
        for operation in ('check', 'package'):
            with self.subTest(operation=operation):
                result = subprocess.run(['bash', '-c', script, 'test', str(output / 'PKGBUILD'),
                                         str(self.root / 'src'), str(binaries), operation],
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, 'ctest-real-identity\n')


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
