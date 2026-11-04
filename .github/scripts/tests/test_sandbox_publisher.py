"""Offline release-resolution and distribution checks; never upload packages."""

import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile


ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "sandbox_publisher", ROOT / ".github/scripts/resolve_sandbox_release.py"
)
publisher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publisher)
TAG = "sandbox-v1.2.3"
SHA = "a" * 40
RELEASE = {
    "tag_name": TAG,
    "draft": False,
    "prerelease": False,
    "published_at": "2026-09-09T00:00:00Z",
}


class SandboxPublisherTests(unittest.TestCase):
    def test_stable_tag_only(self):
        self.assertEqual(publisher.version_from_tag(TAG), "1.2.3")
        for tag in [
            "1.2.3",
            "sandbox-v1.2.3-rc.1",
            "sandbox-v1.2.3+meta",
            "sandbox-v01.2.3",
            "sandbox-v1.2.3\n",
            "sandbox-v1.2.3junk",
            "cua-driver-rs-v1.2.3",
            "sandbox-v$(echo bad)",
        ]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                publisher.version_from_tag(tag)

    def test_published_matching_release_required(self):
        self.assertEqual(publisher.validate_release(TAG, RELEASE), "1.2.3")
        for field, value in [
            ("draft", True),
            ("prerelease", True),
            ("tag_name", "sandbox-v1.2.4"),
            ("published_at", None),
            ("draft", None),
            ("prerelease", None),
        ]:
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                publisher.validate_release(TAG, {**RELEASE, field: value})

    def test_versions_match(self):
        manifest = {publisher.PACKAGE_PATH: "1.2.3"}
        project = {"name": "cua-sandbox", "version": "1.2.3"}
        publisher.validate_versions("1.2.3", manifest, project)
        for bad_manifest, bad_project in [
            ({}, project),
            ({publisher.PACKAGE_PATH: "1.2.2"}, project),
            (manifest, {**project, "version": "1.2.4"}),
            (manifest, {**project, "name": "another-package"}),
        ]:
            with self.subTest(manifest=bad_manifest, project=bad_project):
                with self.assertRaises(ValueError):
                    publisher.validate_versions("1.2.3", bad_manifest, bad_project)

    def resolve_command(self, *args):
        if args[:2] == ("gh", "api"):
            return json.dumps(RELEASE)
        if args == ("git", "rev-parse", "--is-shallow-repository"):
            return "true"
        if args[:2] == ("git", "fetch") or args[:2] == ("git", "merge-base"):
            return ""
        if args == ("git", "rev-parse", f"refs/tags/{TAG}^{{commit}}"):
            return SHA
        if args == ("git", "show", f"{SHA}:.release-please-manifest.json"):
            return json.dumps({publisher.PACKAGE_PATH: "1.2.3"})
        if args == ("git", "show", f"{SHA}:{publisher.PACKAGE_PATH}/pyproject.toml"):
            return '[project]\nname = "cua-sandbox"\nversion = "1.2.3"\n'
        if args == ("git", "show", f"{SHA}:{publisher.PACKAGE_PATH}/VERSION"):
            return "1.2.3"
        if args == ("git", "show", f"{SHA}:{publisher.PACKAGE_PATH}/cua_sandbox/__init__.py"):
            return '__version__ = "1.2.3"  # x-release-please-version\n'
        if args == ("git", "show", f"{SHA}:{publisher.PACKAGE_PATH}/uv.lock"):
            return '[[package]]\nname = "cua-sandbox"\nversion = "1.2.3"\nsource = { editable = "." }\n'
        self.fail(f"Unexpected command: {args}")

    def test_all_source_versions_must_match(self):
        lock = {
            "package": [{"name": "cua-sandbox", "version": "1.2.3", "source": {"editable": "."}}]
        }
        runtime = '__version__ = "1.2.3"'
        publisher.validate_source_versions("1.2.3", "1.2.3\n", runtime, lock)
        for authority, source, locked in [
            ("1.2.2", runtime, lock),
            ("1.2.3", '__version__ = "1.2.2"', lock),
            ("1.2.3", "", lock),
            ("1.2.3", runtime, {"package": []}),
            ("1.2.3", runtime, {"package": lock["package"] * 2}),
            ("1.2.3", runtime, {"package": [{**lock["package"][0], "version": "1.2.2"}]}),
        ]:
            with self.subTest(authority=authority, runtime=source, lock=locked):
                with self.assertRaises(ValueError):
                    publisher.validate_source_versions("1.2.3", authority, source, locked)

    def test_resolves_peeled_tag_and_reads_versions_at_sha(self):
        with patch.object(publisher, "command", side_effect=self.resolve_command) as command:
            self.assertEqual(publisher.resolve(TAG, "example/repository"), ("1.2.3", SHA))
        command.assert_any_call(
            "git", "merge-base", "--is-ancestor", SHA, "refs/remotes/origin/main"
        )
        command.assert_any_call(
            "git",
            "fetch",
            "--no-tags",
            "--unshallow",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main",
            f"+refs/tags/{TAG}:refs/tags/{TAG}",
        )

    def test_non_shallow_repository(self):
        def command(*args):
            if args == ("git", "rev-parse", "--is-shallow-repository"):
                return "false"
            return self.resolve_command(*args)

        with patch.object(publisher, "command", side_effect=command) as mock:
            publisher.resolve(TAG, "example/repository")
        mock.assert_any_call(
            "git",
            "fetch",
            "--no-tags",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main",
            f"+refs/tags/{TAG}:refs/tags/{TAG}",
        )

    def test_invalid_tag_does_not_run_commands(self):
        with patch.object(publisher, "command") as command, self.assertRaises(ValueError):
            publisher.resolve("sandbox-v1.2.3; echo bad", "example/repository")
        command.assert_not_called()

    def test_missing_release_fails(self):
        with patch.object(publisher, "command", side_effect=subprocess.CalledProcessError(1, "gh")):
            with self.assertRaises(subprocess.CalledProcessError):
                publisher.resolve(TAG, "example/repository")

    def test_non_main_commit_fails_before_reading_source(self):
        def command(*args):
            if args[:2] == ("git", "merge-base"):
                raise subprocess.CalledProcessError(1, args)
            return self.resolve_command(*args)

        with patch.object(publisher, "command", side_effect=command) as mock:
            with self.assertRaises(subprocess.CalledProcessError):
                publisher.resolve(TAG, "example/repository")
        self.assertFalse(any(call.args[:2] == ("git", "show") for call in mock.call_args_list))

    def test_metadata_requires_exact_name_and_version(self):
        publisher.validate_metadata(b"Name: cua_sandbox\nVersion: 1.2.3\n", "1.2.3")
        for raw in [
            b"Name: other\nVersion: 1.2.3\n",
            b"Name: cua-sandbox\nVersion: 1.2.4\n",
            b"Name: cua-sandbox\n",
            b"Name: cua-sandbox\nName: other\nVersion: 1.2.3\n",
            b"Name: cua-sandbox\nVersion: 1.2.3\nVersion: 1.2.3\n",
        ]:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                publisher.validate_metadata(raw, "1.2.3")

    def make_artifacts(self, directory, wheel_metadata=None, sdist_metadata=None):
        raw = b"Name: cua-sandbox\nVersion: 1.2.3\n"
        with zipfile.ZipFile(directory / "cua_sandbox-1.2.3-py3-none-any.whl", "w") as wheel:
            wheel.writestr(
                "cua_sandbox-1.2.3.dist-info/METADATA",
                raw if wheel_metadata is None else wheel_metadata,
            )
        with tarfile.open(directory / "cua_sandbox-1.2.3.tar.gz", "w:gz") as sdist:
            data = raw if sdist_metadata is None else sdist_metadata
            entry = tarfile.TarInfo("cua_sandbox-1.2.3/PKG-INFO")
            entry.size = len(data)
            sdist.addfile(entry, io.BytesIO(data))

    def test_artifact_pair(self):
        with tempfile.TemporaryDirectory() as directory:
            self.make_artifacts(Path(directory))
            publisher.validate_artifacts(directory, "1.2.3")

    def test_each_artifact_metadata_is_checked(self):
        for field in ["wheel_metadata", "sdist_metadata"]:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                self.make_artifacts(Path(directory), **{field: b"Name: other\nVersion: 1.2.3\n"})
                with self.assertRaises(ValueError):
                    publisher.validate_artifacts(directory, "1.2.3")

    def test_missing_or_extra_artifacts_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                publisher.validate_artifacts(directory, "1.2.3")
            self.make_artifacts(Path(directory))
            (Path(directory) / "unexpected.txt").touch()
            with self.assertRaises(ValueError):
                publisher.validate_artifacts(directory, "1.2.3")

    def test_workflow_contract(self):
        workflow = (ROOT / ".github/workflows/cd-py-sandbox.yml").read_text()
        self.assertIn("types: [published]", workflow)
        self.assertIn("ref: ${{ steps.release.outputs.sha }}", workflow)
        self.assertIn("python -m twine check --strict", workflow)
        self.assertIn("contents: read", workflow)
        for forbidden in [
            "workflow_call:",
            "py-reusable-publish.yml",
            "create-release:",
            "--skip-existing",
            "--clobber",
            "contents: write",
        ]:
            self.assertNotIn(forbidden, workflow)
        self.assertEqual(workflow.count("secrets.PYPI_TOKEN"), 1)
        self.assertLess(
            workflow.index("Verify distribution metadata"), workflow.index("secrets.PYPI_TOKEN")
        )


if __name__ == "__main__":
    unittest.main()
