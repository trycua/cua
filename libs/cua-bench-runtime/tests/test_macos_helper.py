from __future__ import annotations

import hashlib
import importlib.machinery
import importlib.util
import io
import json
import os
import stat
import tarfile
import tempfile
import types
import unittest
import zipfile
from pathlib import Path
from unittest import mock

if os.name == "nt":
    raise unittest.SkipTest("macOS helper tests require POSIX modules")


HELPER_PATH = Path(__file__).parents[1] / "guest/macos/cdb-helper"


def load_helper():
    name = "cdb_helper_under_test"
    loader = importlib.machinery.SourceFileLoader(name, str(HELPER_PATH))
    spec = importlib.util.spec_from_loader(name, loader)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


helper = load_helper()


class MacOSHelperTests(unittest.TestCase):
    def test_shutdown_dispatch_execs_fixed_system_command(self) -> None:
        with (
            mock.patch.object(helper.os, "geteuid", return_value=0),
            mock.patch.dict(os.environ, {"SUDO_USER": "lume"}, clear=False),
            mock.patch.object(helper.os, "execve") as execve,
        ):
            self.assertEqual(helper.dispatch(["shutdown"]), 0)
        execve.assert_called_once_with(
            "/sbin/shutdown",
            ("/sbin/shutdown", "-h", "now"),
            {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "C"},
        )

        with (
            mock.patch.object(helper.os, "geteuid", return_value=0),
            mock.patch.dict(os.environ, {"SUDO_USER": "lume"}, clear=False),
            self.assertRaises(helper.HelperError),
        ):
            helper.dispatch(["shutdown", "now"])

    def _chrome_zip(self, members: list[tuple[str, bytes, int | None]]) -> bytes:
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, content, mode in members:
                info = zipfile.ZipInfo(name)
                info.compress_type = zipfile.ZIP_DEFLATED
                if mode is not None:
                    info.create_system = 3
                    info.external_attr = mode << 16
                archive.writestr(info, content)
        return output.getvalue()

    def test_chrome_archive_accepts_only_bounded_single_bundle_tree(self) -> None:
        valid = self._chrome_zip(
            [
                ("Google Chrome.app/", b"", stat.S_IFDIR | 0o755),
                ("Google Chrome.app/Contents/", b"", stat.S_IFDIR | 0o755),
                ("Google Chrome.app/Contents/file", b"payload", stat.S_IFREG | 0o644),
                ("Google Chrome.app/Versions/Current", b"151", stat.S_IFLNK | 0o777),
            ]
        )
        helper._chrome_archive_members(io.BytesIO(valid))

        cases = {
            "traversal": [("Google Chrome.app/../escape", b"x", stat.S_IFREG | 0o644)],
            "absolute": [("/Google Chrome.app/file", b"x", stat.S_IFREG | 0o644)],
            "other-root": [("Other.app/file", b"x", stat.S_IFREG | 0o644)],
            "case-alias": [
                ("Google Chrome.app/A", b"x", stat.S_IFREG | 0o644),
                ("Google Chrome.app/a", b"y", stat.S_IFREG | 0o644),
            ],
            "escaping-link": [("Google Chrome.app/link", b"../../outside", stat.S_IFLNK | 0o777)],
            "unsupported-type": [("Google Chrome.app/socket", b"", stat.S_IFSOCK | 0o600)],
        }
        for label, members in cases.items():
            with self.subTest(label=label), self.assertRaises(helper.HelperError):
                helper._chrome_archive_members(io.BytesIO(self._chrome_zip(members)))

    def test_chrome_parent_hardens_exact_standard_seed_state(self) -> None:
        seed = types.SimpleNamespace(
            st_dev=1,
            st_ino=2,
            st_mode=stat.S_IFDIR | 0o775,
            st_uid=0,
            st_gid=80,
        )
        hardened = types.SimpleNamespace(
            st_dev=1,
            st_ino=2,
            st_mode=stat.S_IFDIR | 0o755,
            st_uid=0,
            st_gid=0,
        )
        operations = mock.Mock()
        with (
            mock.patch.object(helper, "_root_owned_unwritable"),
            mock.patch.object(
                helper.grp,
                "getgrnam",
                side_effect=lambda name: types.SimpleNamespace(
                    gr_gid={"admin": 80, "wheel": 0}[name]
                ),
            ),
            mock.patch.object(helper.os, "open", return_value=41),
            mock.patch.object(helper.os, "fstat", side_effect=[seed, hardened]),
            mock.patch.object(helper.Path, "lstat", side_effect=[seed, hardened]),
            mock.patch.object(helper, "_chrome_parent_metadata_is_clean", return_value=True),
            mock.patch.object(helper.os, "fchown") as fchown,
            mock.patch.object(helper.os, "fchmod") as fchmod,
            mock.patch.object(helper.os, "fsync") as fsync,
            mock.patch.object(helper.os, "close"),
        ):
            operations.attach_mock(fchown, "chown")
            operations.attach_mock(fchmod, "chmod")
            operations.attach_mock(fsync, "fsync")
            self.assertTrue(helper._validate_chrome_parent())
        self.assertEqual(
            operations.mock_calls,
            [mock.call.chown(41, 0, 0), mock.call.chmod(41, 0o755), mock.call.fsync(41)],
        )

    def test_chrome_parent_accepts_only_seed_or_hardened_state(self) -> None:
        for label, uid, gid, mode, accepted in (
            ("hardened", 0, 0, 0o755, True),
            ("owner", 501, 80, 0o775, False),
            ("group", 0, 20, 0o775, False),
            ("mode", 0, 80, 0o755, False),
        ):
            status = types.SimpleNamespace(
                st_dev=1,
                st_ino=2,
                st_mode=stat.S_IFDIR | mode,
                st_uid=uid,
                st_gid=gid,
            )
            with (
                self.subTest(label=label),
                mock.patch.object(helper, "_root_owned_unwritable"),
                mock.patch.object(
                    helper.grp,
                    "getgrnam",
                    side_effect=lambda name: types.SimpleNamespace(
                        gr_gid={"admin": 80, "wheel": 0}[name]
                    ),
                ),
                mock.patch.object(helper.os, "open", return_value=41),
                mock.patch.object(helper.os, "fstat", side_effect=[status, status]),
                mock.patch.object(helper.Path, "lstat", side_effect=[status, status]),
                mock.patch.object(helper, "_chrome_parent_metadata_is_clean", return_value=True),
                mock.patch.object(helper.os, "fchown") as fchown,
                mock.patch.object(helper.os, "close"),
            ):
                if accepted:
                    self.assertFalse(helper._validate_chrome_parent())
                else:
                    with self.assertRaisesRegex(helper.HelperError, "unsupported"):
                        helper._validate_chrome_parent()
                fchown.assert_not_called()

    def test_chrome_parent_rejects_acl_and_xattr_before_mutation(self) -> None:
        clean = types.SimpleNamespace(returncode=0, stdout="", stderr="")
        xattrs = types.SimpleNamespace(returncode=0, stdout="com.apple.quarantine\n", stderr="")
        acl = types.SimpleNamespace(
            returncode=0, stdout="drwxrwxr-x+ 3 root admin 96 /Applications\n", stderr=""
        )
        seed = types.SimpleNamespace(
            st_dev=1, st_ino=2, st_mode=stat.S_IFDIR | 0o775, st_uid=0, st_gid=80
        )
        for label, results in (
            ("xattr", [xattrs, clean]),
            ("acl", [clean, acl]),
        ):
            with (
                self.subTest(label=label),
                mock.patch.object(helper, "_root_owned_unwritable"),
                mock.patch.object(
                    helper.grp,
                    "getgrnam",
                    side_effect=lambda name: types.SimpleNamespace(
                        gr_gid={"admin": 80, "wheel": 0}[name]
                    ),
                ),
                mock.patch.object(helper.os, "open", return_value=41),
                mock.patch.object(helper.os, "fstat", return_value=seed),
                mock.patch.object(helper.Path, "lstat", return_value=seed),
                mock.patch.object(helper.subprocess, "run", side_effect=results),
                mock.patch.object(helper.os, "fchown") as fchown,
                mock.patch.object(helper.os, "close"),
                self.assertRaisesRegex(helper.HelperError, "unsafe"),
            ):
                helper._validate_chrome_parent()
            fchown.assert_not_called()

    def test_chrome_parent_stabilizes_one_time_mutation_failure(self) -> None:
        seed = types.SimpleNamespace(
            st_dev=1, st_ino=2, st_mode=stat.S_IFDIR | 0o775, st_uid=0, st_gid=80
        )
        hardened = types.SimpleNamespace(
            st_dev=1, st_ino=2, st_mode=stat.S_IFDIR | 0o755, st_uid=0, st_gid=0
        )
        with (
            mock.patch.object(helper, "_root_owned_unwritable"),
            mock.patch.object(
                helper.grp,
                "getgrnam",
                side_effect=lambda name: types.SimpleNamespace(
                    gr_gid={"admin": 80, "wheel": 0}[name]
                ),
            ),
            mock.patch.object(helper.os, "open", return_value=41),
            mock.patch.object(helper.os, "fstat", side_effect=[seed, hardened]),
            mock.patch.object(helper.Path, "lstat", side_effect=[seed, hardened]),
            mock.patch.object(helper, "_chrome_parent_metadata_is_clean", return_value=True),
            mock.patch.object(helper.os, "fchown") as fchown,
            mock.patch.object(helper.os, "fchmod", side_effect=[OSError("once"), None]) as fchmod,
            mock.patch.object(helper.os, "fsync") as fsync,
            mock.patch.object(helper.os, "close"),
            self.assertRaisesRegex(helper.HelperError, "hardening failed"),
        ):
            helper._validate_chrome_parent()
        self.assertEqual(fchown.call_count, 2)
        self.assertEqual(fchmod.call_count, 2)
        fsync.assert_called_once_with(41)

    def test_chrome_source_parent_requires_exact_fd_bound_profile(self) -> None:
        def profile_status(index: int, uid: int, gid: int, mode: int):
            return types.SimpleNamespace(
                st_dev=1,
                st_ino=100 + index,
                st_mode=stat.S_IFDIR | mode,
                st_uid=uid,
                st_gid=gid,
            )

        statuses = [
            profile_status(0, 0, 80, 0o755),
            profile_status(1, 99, 99, 0o755),
            profile_status(2, 99, 99, 0o755),
            profile_status(3, 501, 20, 0o700),
            profile_status(4, 501, 20, 0o700),
        ]
        with (
            mock.patch.object(helper, "_root_owned_unwritable"),
            mock.patch.object(
                helper.grp,
                "getgrnam",
                side_effect=lambda name: types.SimpleNamespace(
                    gr_gid={"admin": 80, "staff": 20}[name]
                ),
            ),
            mock.patch.object(helper.os, "open", side_effect=range(41, 46)) as opened,
            mock.patch.object(helper.os, "fstat", side_effect=statuses),
            mock.patch.object(helper.Path, "lstat", side_effect=statuses),
            mock.patch.object(helper.os, "close") as closed,
        ):
            helper._validate_chrome_source_parent(501)
        self.assertEqual(opened.call_count, 5)
        self.assertEqual(closed.call_count, 5)

        with (
            mock.patch.object(helper, "_root_owned_unwritable"),
            self.assertRaisesRegex(helper.HelperError, "control account identity mismatch"),
        ):
            helper._validate_chrome_source_parent(502)

        mismatched = list(statuses)
        mismatched[3] = profile_status(3, 501, 20, 0o755)
        with (
            mock.patch.object(helper, "_root_owned_unwritable"),
            mock.patch.object(
                helper.grp,
                "getgrnam",
                side_effect=lambda name: types.SimpleNamespace(
                    gr_gid={"admin": 80, "staff": 20}[name]
                ),
            ),
            mock.patch.object(helper.os, "open", side_effect=range(41, 46)),
            mock.patch.object(helper.os, "fstat", side_effect=mismatched),
            mock.patch.object(helper.Path, "lstat", side_effect=mismatched),
            mock.patch.object(helper.os, "close"),
            self.assertRaisesRegex(helper.HelperError, "profile 3 mode mismatch 0755/0700"),
        ):
            helper._validate_chrome_source_parent(501)

    def test_chrome_archive_rejects_member_and_expansion_bounds(self) -> None:
        archive = self._chrome_zip(
            [("Google Chrome.app/file", b"0123456789", stat.S_IFREG | 0o644)]
        )
        with (
            mock.patch.object(helper, "MAX_CHROME_MEMBER_BYTES", 9),
            self.assertRaisesRegex(helper.HelperError, "member is too large"),
        ):
            helper._chrome_archive_members(io.BytesIO(archive))
        with (
            mock.patch.object(helper, "MAX_CHROME_EXPANDED_BYTES", 9),
            self.assertRaisesRegex(helper.HelperError, "expansion is too large"),
        ):
            helper._chrome_archive_members(io.BytesIO(archive))

    def test_chrome_tree_digest_covers_content_links_and_modes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "Google Chrome.app"
            root.mkdir(mode=0o755)
            binary = root / "binary"
            binary.write_bytes(b"one")
            binary.chmod(0o755)
            (root / "link").symlink_to("binary")
            initial = helper._chrome_tree_digest(root)
            binary.write_bytes(b"two")
            self.assertNotEqual(initial, helper._chrome_tree_digest(root))
            binary.write_bytes(b"one")
            binary.chmod(0o700)
            self.assertNotEqual(initial, helper._chrome_tree_digest(root))
            binary.chmod(0o755)
            (root / "link").unlink()
            (root / "link").symlink_to("other")
            self.assertNotEqual(initial, helper._chrome_tree_digest(root))

    def test_chrome_stage_recovery_is_exact_bounded_and_identity_checked(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            stage = Path(directory) / ".cdb-google-chrome-stage"
            stage.mkdir(mode=0o700)
            (stage / "partial").write_bytes(b"owned partial")
            status = stage.lstat()
            with (
                mock.patch.object(helper, "CHROME_STAGE_PARENT", stage),
                mock.patch.object(helper, "CHROME_STAGE_UID", status.st_uid),
                mock.patch.object(helper, "CHROME_STAGE_GID", status.st_gid),
                mock.patch.object(helper, "_validate_chrome_tree_security") as validate,
            ):
                self.assertTrue(helper._recover_chrome_stage())
            validate.assert_called_once_with(stage)
            self.assertFalse(stage.exists())

            target = Path(directory) / "other"
            target.mkdir()
            stage.symlink_to(target, target_is_directory=True)
            with (
                mock.patch.object(helper, "CHROME_STAGE_PARENT", stage),
                self.assertRaisesRegex(helper.HelperError, "recovery path is unsafe"),
            ):
                helper._recover_chrome_stage()
            self.assertTrue(target.is_dir())

    def test_chrome_bundle_requires_version_signing_and_universal_binary(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / "Google Chrome.app"
            binary = app / "Contents/MacOS/Google Chrome"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"binary")
            with (app / "Contents/Info.plist").open("wb") as stream:
                import plistlib

                plistlib.dump(
                    {
                        "CFBundleIdentifier": helper.CHROME_IDENTIFIER,
                        "CFBundleShortVersionString": helper.CHROME_VERSION,
                    },
                    stream,
                )
            valid_runs = [
                types.SimpleNamespace(returncode=0, stdout="", stderr=""),
                types.SimpleNamespace(
                    returncode=0,
                    stdout="",
                    stderr="Identifier=com.google.Chrome\nTeamIdentifier=EQHXZ8M8AV\n",
                ),
                types.SimpleNamespace(returncode=0, stdout="x86_64 arm64\n", stderr=""),
            ]
            with (
                mock.patch.object(helper, "_validate_chrome_tree_security"),
                mock.patch.object(
                    helper, "_chrome_tree_digest", return_value=helper.CHROME_TREE_SHA256
                ),
                mock.patch.object(helper.subprocess, "run", side_effect=valid_runs) as run,
            ):
                helper._validate_chrome_bundle(app)
            self.assertEqual(run.call_args_list[0].args[0][1:4], ["--verify", "--deep", "--strict"])

            for label, runs in (
                (
                    "signature",
                    [types.SimpleNamespace(returncode=1, stdout="", stderr=""), *valid_runs[1:]],
                ),
                (
                    "identity",
                    [
                        valid_runs[0],
                        types.SimpleNamespace(
                            returncode=0,
                            stdout="",
                            stderr="Identifier=other\nTeamIdentifier=OTHER\n",
                        ),
                        valid_runs[2],
                    ],
                ),
                (
                    "architecture",
                    [
                        *valid_runs[:2],
                        types.SimpleNamespace(returncode=0, stdout="arm64\n", stderr=""),
                    ],
                ),
            ):
                with (
                    self.subTest(label=label),
                    mock.patch.object(helper, "_validate_chrome_tree_security"),
                    mock.patch.object(
                        helper, "_chrome_tree_digest", return_value=helper.CHROME_TREE_SHA256
                    ),
                    mock.patch.object(helper.subprocess, "run", side_effect=runs),
                    self.assertRaises(helper.HelperError),
                ):
                    helper._validate_chrome_bundle(app)

    def test_chrome_existing_mismatch_fails_before_reading_inbox(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "Google Chrome.app"
            target.mkdir()
            with (
                mock.patch.object(helper, "CHROME_TARGET", target),
                mock.patch.object(helper, "_validate_chrome_parent", return_value=True),
                mock.patch.object(
                    helper.pwd, "getpwnam", return_value=types.SimpleNamespace(pw_uid=os.getuid())
                ),
                mock.patch.object(
                    helper.grp, "getgrnam", return_value=types.SimpleNamespace(gr_gid=os.getgid())
                ),
                mock.patch.object(
                    helper, "_validate_chrome_bundle", side_effect=helper.HelperError("mismatch")
                ),
                mock.patch.object(helper.os, "open") as opened,
                self.assertRaisesRegex(helper.HelperError, "mismatch"),
            ):
                helper.provision_chrome()
            opened.assert_not_called()

    def test_chrome_provision_rejects_unsafe_archive_identity(self) -> None:
        archive = self._chrome_zip([("Google Chrome.app/file", b"payload", stat.S_IFREG | 0o644)])
        for case in ("mode", "symlink", "hardlink", "digest"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "inbox/chrome.zip"
                target = root / "Applications/Google Chrome.app"
                stage = root / "Applications/.stage"
                source.parent.mkdir()
                target.parent.mkdir()
                source.write_bytes(archive)
                source.chmod(0o600 if case != "mode" else 0o644)
                if case == "symlink":
                    actual = root / "actual.zip"
                    source.rename(actual)
                    source.symlink_to(actual)
                elif case == "hardlink":
                    os.link(source, root / "second-link")
                expected_digest = (
                    "f" * 64 if case == "digest" else hashlib.sha256(archive).hexdigest()
                )
                with (
                    mock.patch.object(helper, "CHROME_ARCHIVE_SOURCE", source),
                    mock.patch.object(helper, "CHROME_TARGET", target),
                    mock.patch.object(helper, "CHROME_STAGE_PARENT", stage),
                    mock.patch.object(helper, "CHROME_ARCHIVE_SHA256", expected_digest),
                    mock.patch.object(helper, "_validate_chrome_parent"),
                    mock.patch.object(helper, "_validate_chrome_source_parent"),
                    mock.patch.object(
                        helper.pwd,
                        "getpwnam",
                        return_value=types.SimpleNamespace(pw_uid=os.getuid()),
                    ),
                    mock.patch.object(
                        helper.grp,
                        "getgrnam",
                        return_value=types.SimpleNamespace(gr_gid=os.getgid()),
                    ),
                    self.assertRaises(helper.HelperError),
                ):
                    helper.provision_chrome()
                self.assertTrue(source.exists())
                self.assertFalse(target.exists())
                self.assertFalse(stage.exists())

    def test_chrome_scrub_clears_xattrs_acls_and_skips_symlink_chmod(self) -> None:
        regular = Path("/fixed/Google Chrome.app")
        symlink = regular / "link"
        regular_status = types.SimpleNamespace(
            st_dev=1,
            st_ino=2,
            st_mode=stat.S_IFDIR | 0o775,
        )
        symlink_status = types.SimpleNamespace(
            st_dev=1,
            st_ino=3,
            st_mode=stat.S_IFLNK | 0o777,
        )

        def lstat(path):
            return symlink_status if path == symlink else regular_status

        completed = types.SimpleNamespace(returncode=0)
        with (
            mock.patch.object(
                helper,
                "_chrome_tree_entries",
                return_value=[(regular, regular_status), (symlink, symlink_status)],
            ),
            mock.patch.object(helper.Path, "lstat", lstat),
            mock.patch.object(helper.subprocess, "run", return_value=completed) as run,
            mock.patch.object(helper.os, "chown") as chown,
            mock.patch.object(helper.os, "chmod") as chmod,
        ):
            helper._scrub_chrome_tree(regular, 0)
        self.assertIn("-s", run.call_args_list[0].args[0])
        self.assertIn("-c", run.call_args_list[0].args[0])
        self.assertEqual(chown.call_count, 2)
        chmod.assert_called_once_with(regular, 0o755)

    def test_chrome_provision_installs_atomically_then_removes_inbox(self) -> None:
        archive = self._chrome_zip([("Google Chrome.app/file", b"payload", stat.S_IFREG | 0o644)])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "inbox/chrome.zip"
            target = root / "Applications/Google Chrome.app"
            stage = root / "Applications/.stage"
            source.parent.mkdir()
            target.parent.mkdir()
            source.write_bytes(archive)
            source.chmod(0o600)

            def extract(*args, **kwargs):
                app = stage / "Google Chrome.app"
                app.mkdir()
                (app / "file").write_bytes(b"payload")
                return types.SimpleNamespace(returncode=0)

            validations = []

            def validate(path):
                validations.append(path)
                if not (path / "file").is_file():
                    raise helper.HelperError("invalid")

            with (
                mock.patch.object(helper, "CHROME_ARCHIVE_SOURCE", source),
                mock.patch.object(helper, "CHROME_TARGET", target),
                mock.patch.object(helper, "CHROME_STAGE_PARENT", stage),
                mock.patch.object(
                    helper, "CHROME_ARCHIVE_SHA256", hashlib.sha256(archive).hexdigest()
                ),
                mock.patch.object(helper, "CHROME_PRIVILEGED_SOURCE_UID", os.getuid()),
                mock.patch.object(helper, "CHROME_PRIVILEGED_SOURCE_GID", source.lstat().st_gid),
                mock.patch.object(helper, "_validate_chrome_parent", return_value=True),
                mock.patch.object(helper, "_validate_chrome_source_parent"),
                mock.patch.object(
                    helper.pwd, "getpwnam", return_value=types.SimpleNamespace(pw_uid=os.getuid())
                ),
                mock.patch.object(
                    helper.grp, "getgrnam", return_value=types.SimpleNamespace(gr_gid=os.getgid())
                ),
                mock.patch.object(helper.subprocess, "run", side_effect=extract),
                mock.patch.object(helper, "_scrub_chrome_tree"),
                mock.patch.object(helper, "_validate_chrome_bundle", side_effect=validate),
                mock.patch.object(
                    helper, "_rename_no_replace", side_effect=lambda old, new: os.rename(old, new)
                ),
            ):
                result = helper.provision_chrome()
            self.assertTrue(result["installed"])
            self.assertTrue(result["applications_parent_hardened"])
            self.assertTrue(result["applications_parent_seed_normalized"])
            self.assertEqual(validations, [stage / "Google Chrome.app", target])
            self.assertEqual((target / "file").read_bytes(), b"payload")
            self.assertFalse(source.exists())
            self.assertFalse(stage.exists())

    def test_chrome_provision_rolls_back_only_its_published_identity(self) -> None:
        archive = self._chrome_zip([("Google Chrome.app/file", b"payload", stat.S_IFREG | 0o644)])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "inbox/chrome.zip"
            target = root / "Applications/Google Chrome.app"
            stage = root / "Applications/.stage"
            source.parent.mkdir()
            target.parent.mkdir()
            source.write_bytes(archive)
            source.chmod(0o600)

            def extract(*args, **kwargs):
                app = stage / "Google Chrome.app"
                app.mkdir()
                (app / "file").write_bytes(b"payload")
                return types.SimpleNamespace(returncode=0)

            with (
                mock.patch.object(helper, "CHROME_ARCHIVE_SOURCE", source),
                mock.patch.object(helper, "CHROME_TARGET", target),
                mock.patch.object(helper, "CHROME_STAGE_PARENT", stage),
                mock.patch.object(
                    helper, "CHROME_ARCHIVE_SHA256", hashlib.sha256(archive).hexdigest()
                ),
                mock.patch.object(helper, "CHROME_PRIVILEGED_SOURCE_UID", os.getuid()),
                mock.patch.object(helper, "CHROME_PRIVILEGED_SOURCE_GID", source.lstat().st_gid),
                mock.patch.object(helper, "_validate_chrome_parent"),
                mock.patch.object(helper, "_validate_chrome_source_parent"),
                mock.patch.object(
                    helper.pwd, "getpwnam", return_value=types.SimpleNamespace(pw_uid=os.getuid())
                ),
                mock.patch.object(
                    helper.grp, "getgrnam", return_value=types.SimpleNamespace(gr_gid=os.getgid())
                ),
                mock.patch.object(helper.subprocess, "run", side_effect=extract),
                mock.patch.object(helper, "_scrub_chrome_tree"),
                mock.patch.object(
                    helper,
                    "_validate_chrome_bundle",
                    side_effect=[None, helper.HelperError("post-publish")],
                ),
                mock.patch.object(
                    helper, "_rename_no_replace", side_effect=lambda old, new: os.rename(old, new)
                ),
                self.assertRaisesRegex(helper.HelperError, "post-publish"),
            ):
                helper.provision_chrome()
            self.assertFalse(target.exists())
            self.assertTrue(source.exists())
            self.assertFalse(stage.exists())

    def test_chrome_dispatch_is_no_argument_only(self) -> None:
        with (
            mock.patch.object(helper.os, "geteuid", return_value=0),
            mock.patch.dict(os.environ, {"SUDO_USER": "lume"}, clear=False),
            mock.patch.object(
                helper, "provision_chrome", return_value={"verb": "provision-chrome"}
            ) as provision,
            mock.patch.object(helper, "emit"),
        ):
            self.assertEqual(helper.dispatch(["provision-chrome"]), 0)
            provision.assert_called_once_with()
            with self.assertRaises(helper.HelperError):
                helper.dispatch(["provision-chrome", "/tmp/archive.zip"])

    def _companion_context(self, root: Path, content: bytes):
        source = root / "inbox/codex-code-mode-host"
        target = root / "bin/codex-code-mode-host"
        staging = root / "bin/.codex-code-mode-host.cdb-staging"
        source.parent.mkdir()
        target.parent.mkdir()
        source.write_bytes(content)

        def validate_target(path, expected_digest, *, expected_mode=None):
            self.assertEqual(path, target)
            self.assertEqual(expected_digest, hashlib.sha256(content).hexdigest())
            self.assertEqual(expected_mode, 0o755)
            status = path.lstat()
            if (
                not stat.S_ISREG(status.st_mode)
                or stat.S_IMODE(status.st_mode) != 0o755
                or hashlib.sha256(path.read_bytes()).hexdigest() != expected_digest
            ):
                raise helper.HelperError("production executable identity mismatch")

        return (
            source,
            target,
            staging,
            (
                mock.patch.object(helper, "CODEX_COMPANION_SOURCE", source),
                mock.patch.object(helper, "CODEX_COMPANION_TARGET", target),
                mock.patch.object(helper, "CODEX_COMPANION_STAGING", staging),
                mock.patch.object(
                    helper, "CODEX_COMPANION_SHA256", hashlib.sha256(content).hexdigest()
                ),
                mock.patch.object(helper, "_validate_codex_companion_parent"),
                mock.patch.object(helper, "_validate_codex_companion_binary"),
                mock.patch.object(helper, "validate_production_executable", validate_target),
                mock.patch.object(helper.os, "fchown"),
                mock.patch.object(
                    helper.pwd,
                    "getpwnam",
                    return_value=types.SimpleNamespace(pw_uid=os.getuid()),
                ),
            ),
        )

    def test_provision_codex_companion_installs_once_and_is_idempotent(self) -> None:
        content = b"pinned companion"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, target, _staging, patches = self._companion_context(root, content)
            with (
                patches[0],
                patches[1],
                patches[2],
                patches[3],
                patches[4],
                patches[5],
                patches[6],
                patches[7],
                patches[8],
            ):
                result = helper.provision_codex_companion()
            self.assertTrue(result["installed"])
            self.assertEqual(target.read_bytes(), content)
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o755)
            self.assertFalse(source.exists())

            source.write_bytes(content)
            with (
                patches[0],
                patches[1],
                patches[2],
                patches[3],
                patches[4],
                patches[5],
                patches[6],
                patches[7],
                patches[8],
            ):
                result = helper.provision_codex_companion()
            self.assertFalse(result["installed"])
            self.assertEqual(target.read_bytes(), content)
            self.assertFalse(source.exists())

    def test_provision_codex_companion_rejects_unsafe_sources(self) -> None:
        content = b"pinned companion"
        for case in ("symlink", "hardlink", "digest"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source, target, _staging, patches = self._companion_context(root, content)
                if case == "symlink":
                    source.unlink()
                    source.symlink_to(root / "elsewhere")
                elif case == "hardlink":
                    os.link(source, root / "second-link")
                else:
                    patches = list(patches)
                    patches[3] = mock.patch.object(helper, "CODEX_COMPANION_SHA256", "0" * 64)
                with (
                    patches[0],
                    patches[1],
                    patches[2],
                    patches[3],
                    patches[4],
                    patches[5],
                    patches[6],
                    patches[7],
                    patches[8],
                    self.assertRaises(helper.HelperError),
                ):
                    helper.provision_codex_companion()
                self.assertFalse(target.exists())
                self.assertTrue(source.is_symlink() if case == "symlink" else source.exists())

    def test_provision_codex_companion_rejects_existing_mismatch(self) -> None:
        content = b"pinned companion"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, target, _staging, patches = self._companion_context(root, content)
            target.write_bytes(b"wrong")
            target.chmod(0o755)
            with (
                patches[0],
                patches[1],
                patches[2],
                patches[3],
                patches[4],
                patches[5],
                patches[6],
                patches[7],
                patches[8],
                self.assertRaisesRegex(helper.HelperError, "identity mismatch"),
            ):
                helper.provision_codex_companion()
            self.assertEqual(target.read_bytes(), b"wrong")
            self.assertTrue(source.exists())

    def test_provision_codex_companion_rolls_back_post_link_failure(self) -> None:
        content = b"pinned companion"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, target, staging, patches = self._companion_context(root, content)
            binary_checks = [
                None,
                helper.HelperError("synthetic post-link validation failure"),
            ]
            patches = list(patches)
            patches[5] = mock.patch.object(
                helper,
                "_validate_codex_companion_binary",
                side_effect=binary_checks,
            )
            with (
                patches[0],
                patches[1],
                patches[2],
                patches[3],
                patches[4],
                patches[5],
                patches[6],
                patches[7],
                patches[8],
                self.assertRaisesRegex(helper.HelperError, "post-link"),
            ):
                helper.provision_codex_companion()
            self.assertFalse(target.exists())
            self.assertTrue(source.exists())
            self.assertFalse(staging.exists())

    def test_provision_codex_companion_detects_source_path_swap(self) -> None:
        content = b"pinned companion"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, target, _staging, patches = self._companion_context(root, content)
            real_lstat = Path.lstat
            calls = 0

            def swapped_lstat(path, *args, **kwargs):
                nonlocal calls
                status = real_lstat(path, *args, **kwargs)
                if path == source:
                    calls += 1
                    if calls >= 2:
                        return types.SimpleNamespace(
                            st_mode=status.st_mode,
                            st_uid=status.st_uid,
                            st_nlink=status.st_nlink,
                            st_size=status.st_size,
                            st_dev=status.st_dev,
                            st_ino=status.st_ino + 1,
                        )
                return status

            with (
                patches[0],
                patches[1],
                patches[2],
                patches[3],
                patches[4],
                patches[5],
                patches[6],
                patches[7],
                patches[8],
                mock.patch.object(Path, "lstat", swapped_lstat),
                self.assertRaisesRegex(helper.HelperError, "source identity mismatch"),
            ):
                helper.provision_codex_companion()
            self.assertFalse(target.exists())
            self.assertTrue(source.exists())

    def test_codex_companion_dispatch_has_no_caller_paths(self) -> None:
        with (
            mock.patch.object(helper.os, "geteuid", return_value=0),
            mock.patch.dict(os.environ, {"SUDO_USER": "lume"}, clear=False),
            mock.patch.object(
                helper,
                "provision_codex_companion",
                return_value={"verb": "provision-codex-companion"},
            ) as provision,
            mock.patch.object(helper, "emit"),
        ):
            self.assertEqual(helper.dispatch(["provision-codex-companion"]), 0)
            provision.assert_called_once_with()
            with self.assertRaises(helper.HelperError):
                helper.dispatch(["provision-codex-companion", "/tmp/source"])

    def test_codex_companion_binary_requires_architecture_and_signing_identity(self) -> None:
        valid = [
            types.SimpleNamespace(
                returncode=0, stdout="Mach-O 64-bit executable arm64\n", stderr=""
            ),
            types.SimpleNamespace(returncode=0, stdout="", stderr=""),
            types.SimpleNamespace(
                returncode=0,
                stdout="",
                stderr=("Identifier=codex-code-mode-host\nTeamIdentifier=2DC432GLL2\n"),
            ),
        ]
        with mock.patch.object(helper.subprocess, "run", side_effect=valid):
            helper._validate_codex_companion_binary(Path("/usr/local/bin/codex-code-mode-host"))
        for label, results in (
            (
                "architecture",
                [types.SimpleNamespace(returncode=0, stdout="ELF arm64", stderr="")],
            ),
            (
                "signature",
                [
                    valid[0],
                    types.SimpleNamespace(returncode=1, stdout="", stderr=""),
                ],
            ),
            (
                "identity",
                [
                    valid[0],
                    valid[1],
                    types.SimpleNamespace(
                        returncode=0,
                        stdout="",
                        stderr="Identifier=other\nTeamIdentifier=OTHER\n",
                    ),
                ],
            ),
        ):
            with (
                self.subTest(label=label),
                mock.patch.object(helper.subprocess, "run", side_effect=results),
                self.assertRaises(helper.HelperError),
            ):
                helper._validate_codex_companion_binary(Path("/fixed"))

    def test_installation_facts_hashes_exact_main_pf_rules_stdout(self) -> None:
        main_rules = 'anchor "com.trycua.cdb/*" all\n'
        installed = types.SimpleNamespace(st_mode=stat.S_IFREG | 0o555, st_uid=0, st_gid=0)
        sudoers = types.SimpleNamespace(st_mode=stat.S_IFREG | 0o440, st_uid=0, st_gid=0)
        denied = types.SimpleNamespace(returncode=1, stdout="", stderr="")
        sudo_listing = types.SimpleNamespace(
            returncode=1, stdout="", stderr="user is not allowed to run sudo"
        )
        run_results = [
            types.SimpleNamespace(stdout="user is not a member of the group", stderr=""),
            types.SimpleNamespace(
                stdout=("passwordauthentication no\nkbdinteractiveauthentication no\n"),
                stderr="",
            ),
            types.SimpleNamespace(stdout=main_rules, stderr=""),
        ]
        with (
            mock.patch.object(
                helper.Path,
                "stat",
                side_effect=[installed, installed, sudoers],
            ),
            mock.patch.object(helper.subprocess, "run", side_effect=[denied, sudo_listing]),
            mock.patch.object(helper, "run", side_effect=run_results) as run,
            mock.patch.object(helper, "sha256", return_value="0" * 64),
        ):
            facts = helper.installation_facts()
        self.assertEqual(
            facts["pf_main_rules_sha256"],
            hashlib.sha256(main_rules.encode()).hexdigest(),
        )
        self.assertEqual(run.call_args_list[-1], mock.call([helper.PFCTL, "-sr"]))

    def test_fixed_dispatch_rejects_shell_and_unknown_verbs(self) -> None:
        with (
            mock.patch.object(helper.os, "geteuid", return_value=0),
            mock.patch.dict(os.environ, {"SUDO_USER": "lume"}, clear=False),
            mock.patch.object(helper, "run") as run,
        ):
            for invocation in (
                ["shell", "id"],
                ["stage-attempt", "safe", "a" * 64, "extra"],
                ["kill-agent", "../escape"],
                ["start-driver", ";id"],
            ):
                with self.subTest(invocation=invocation), self.assertRaises(helper.HelperError):
                    helper.dispatch(invocation)
            run.assert_not_called()

    def test_driver_launch_retries_until_aqua_domain_is_ready(self) -> None:
        console = types.SimpleNamespace(stdout="501")
        launched = types.SimpleNamespace(stdout="")
        with (
            mock.patch.object(
                helper,
                "run",
                side_effect=[console, helper.HelperError("not ready"), launched],
            ) as run,
            mock.patch.object(helper.time, "monotonic", side_effect=[0.0, 1.0]),
            mock.patch.object(helper.time, "sleep") as sleep,
        ):
            evidence = helper.start_driver()
        self.assertEqual(evidence, {"verb": "start-driver", "console_uid": 501})
        self.assertEqual(run.call_count, 3)
        sleep.assert_called_once_with(1.0)

    def test_caller_must_be_root_control_account(self) -> None:
        with mock.patch.object(helper.os, "geteuid", return_value=501):
            with self.assertRaisesRegex(helper.HelperError, "root"):
                helper.require_root_caller()
        with (
            mock.patch.object(helper.os, "geteuid", return_value=0),
            mock.patch.dict(os.environ, {"SUDO_USER": "cdb-agent"}, clear=False),
        ):
            with self.assertRaisesRegex(helper.HelperError, "control account"):
                helper.require_root_caller()

    def test_pf_none_blocks_agent_tcp_and_udp(self) -> None:
        account = types.SimpleNamespace(pw_uid=502)
        with mock.patch.object(helper.pwd, "getpwnam", return_value=account):
            rules, digest = helper.render_pf("none", [])
        self.assertEqual(
            rules,
            "block drop out quick proto tcp from any to any port = 53\n"
            "block drop out quick proto udp from any to any port = 53\n"
            "block drop out quick proto udp from any to any port = 5353\n"
            "block drop out quick inet proto icmp from any to any\n"
            "block drop out quick inet6 proto ipv6-icmp from any to any\n"
            "block drop out quick inet proto tcp from any to any user = 502\n"
            "block drop out quick inet proto udp from any to any user = 502\n"
            "block drop out quick inet6 proto tcp from any to any user = 502\n"
            "block drop out quick inet6 proto udp from any to any user = 502\n",
        )
        self.assertEqual(digest, hashlib.sha256(b"[]").hexdigest())

    def test_pf_allowlist_is_literal_sorted_and_default_deny(self) -> None:
        account = types.SimpleNamespace(pw_uid=502)
        with mock.patch.object(helper.pwd, "getpwnam", return_value=account):
            rules, _digest = helper.render_pf("allowlist", ["203.0.113.9@443", "198.51.100.2@8443"])
            for invalid in (
                ["provider.example@443"],
                ["127.0.0.1@443"],
                ["203.0.113.9@0"],
                ["203.0.113.9@53"],
                ["203.0.113.9@443\npass out all"],
            ):
                with self.subTest(invalid=invalid), self.assertRaises(helper.HelperError):
                    helper.render_pf("allowlist", invalid)
        self.assertLess(rules.index("198.51.100.2"), rules.index("203.0.113.9"))
        self.assertTrue(
            rules.endswith("block drop out quick inet6 proto udp from any to any user = 502\n")
        )

    def test_safe_archive_rejects_links_and_traversal(self) -> None:
        ordinary = tarfile.TarInfo("task/brief.md")
        ordinary.size = 1
        self.assertEqual(str(helper.safe_member(ordinary)), "task/brief.md")
        for name, kind in (
            ("../escape", tarfile.REGTYPE),
            ("/absolute", tarfile.REGTYPE),
            ("workspace/link", tarfile.SYMTYPE),
            ("workspace/hard", tarfile.LNKTYPE),
        ):
            member = tarfile.TarInfo(name)
            member.type = kind
            with self.subTest(name=name), self.assertRaises(helper.HelperError):
                helper.safe_member(member)

    def test_stage_binds_name_digest_and_removes_payload(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            attempts = root / "attempts"
            scratch = root / "scratch"
            attempts.mkdir()
            scratch.mkdir()
            archive = scratch / "cdb-trial-one-payload.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                for name in ("home", "workspace", "artifacts"):
                    info = tarfile.TarInfo(name)
                    info.type = tarfile.DIRTYPE
                    bundle.addfile(info)
                content = b"brief"
                info = tarfile.TarInfo("task/brief.md")
                info.size = len(content)
                bundle.addfile(info, io.BytesIO(content))
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            with (
                mock.patch.object(helper, "ATTEMPT_BASE", attempts),
                mock.patch.object(helper, "SCRATCH_BASE", scratch),
                mock.patch.object(helper, "require_attempt_base"),
                mock.patch.object(helper, "prepare_permissions"),
            ):
                evidence = helper.stage_attempt("trial-one", digest)
            self.assertEqual(evidence["archive_sha256"], digest)
            self.assertTrue((attempts / "trial-one/task/brief.md").is_file())
            self.assertFalse(archive.exists())

    def test_launch_envelope_cannot_escape_attempt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "harness").mkdir()
            envelope = root / "harness/launch.json"
            envelope.write_text(
                json.dumps(
                    {
                        "argv": ["/bin/echo", "bad"],
                        "cwd": str(root),
                        "environment": {"HOME": str(root)},
                    }
                ),
                encoding="utf-8",
            )
            envelope.chmod(0o440)
            protected = types.SimpleNamespace(st_mode=stat.S_IFREG | 0o440, st_uid=0)
            with mock.patch.object(Path, "lstat", return_value=protected):
                with self.assertRaisesRegex(helper.HelperError, "escaped"):
                    helper.load_launch(root)

    def test_python_interpreter_requires_root_protected_staged_script(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "harness/agent/reference.py"
            script.parent.mkdir(parents=True)
            script.write_text("print('ok')\n", encoding="utf-8")
            script.chmod(0o440)
            workspace = root / "workspace"
            workspace.mkdir()
            envelope = root / "harness/launch.json"
            envelope.write_text(
                json.dumps(
                    {
                        "argv": ["/usr/bin/python3", str(script)],
                        "cwd": str(workspace),
                        "environment": {"HOME": str(root / "home")},
                    }
                ),
                encoding="utf-8",
            )
            envelope.chmod(0o440)
            real_stat = Path.stat

            def protected_status(path, *args, **kwargs):
                status = real_stat(path, *args, **kwargs)
                if path.name == "reference.py" and path.parent.name == "agent":
                    return types.SimpleNamespace(
                        st_mode=status.st_mode,
                        st_uid=0,
                    )
                return status

            real_lstat = Path.lstat

            def protected_lstat(path, *args, **kwargs):
                status = real_lstat(path, *args, **kwargs)
                if path == envelope:
                    return types.SimpleNamespace(st_mode=status.st_mode, st_uid=0)
                return status

            with (
                mock.patch.object(Path, "stat", protected_status),
                mock.patch.object(Path, "lstat", protected_lstat),
            ):
                (
                    argv,
                    _environment,
                    cwd,
                    stdin_path,
                    credential_names,
                    executable_sha256,
                    support_executables,
                ) = helper.load_launch(root)
            self.assertEqual(argv[0], "/usr/bin/python3")
            self.assertEqual(cwd, workspace)
            self.assertIsNone(stdin_path)
            self.assertEqual(credential_names, ())
            self.assertIsNone(executable_sha256)
            self.assertEqual(support_executables, ())

    def test_launch_stdin_accepts_control_owned_brief_under_attempt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "agent"
            executable.write_text("#!/bin/sh\n", encoding="utf-8")
            workspace = root / "workspace"
            workspace.mkdir()
            brief = root / "task/brief.md"
            brief.parent.mkdir()
            brief.write_text("benchmark brief\n", encoding="utf-8")
            envelope = root / "harness/launch.json"
            envelope.parent.mkdir()
            envelope.write_text(
                json.dumps(
                    {
                        "argv": [str(executable)],
                        "cwd": str(workspace),
                        "environment": {},
                        "stdin_path": str(brief),
                    }
                ),
                encoding="utf-8",
            )
            envelope.chmod(0o440)
            account = types.SimpleNamespace(pw_uid=os.getuid())
            real_lstat = Path.lstat

            def protected_lstat(path, *args, **kwargs):
                status = real_lstat(path, *args, **kwargs)
                if path == envelope:
                    return types.SimpleNamespace(st_mode=status.st_mode, st_uid=0)
                return status

            with mock.patch.object(Path, "lstat", protected_lstat):
                (
                    _argv,
                    _environment,
                    _cwd,
                    stdin_path,
                    credential_names,
                    executable_sha256,
                    support_executables,
                ) = helper.load_launch(root)
            self.assertEqual(stdin_path, brief)
            self.assertEqual(credential_names, ())
            self.assertIsNone(executable_sha256)
            self.assertEqual(support_executables, ())
            with mock.patch.object(helper.pwd, "getpwnam", return_value=account):
                descriptor = helper.open_agent_stdin(root, stdin_path)
            self.assertIsNotNone(descriptor)
            assert descriptor is not None
            try:
                self.assertEqual(os.read(descriptor, 4096), b"benchmark brief\n")
            finally:
                os.close(descriptor)

    def test_launch_stdin_rejects_symlink_oversize_and_escape(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "attempt"
            root.mkdir()
            brief = root / "brief.md"
            brief.write_text("brief", encoding="utf-8")
            link = root / "brief-link.md"
            link.symlink_to(brief)
            oversize = root / "oversize.md"
            with oversize.open("wb") as output:
                output.truncate(helper.MAX_AGENT_STDIN_BYTES + 1)
            outside = base / "outside.md"
            outside.write_text("outside", encoding="utf-8")
            account = types.SimpleNamespace(pw_uid=os.getuid())
            with mock.patch.object(helper.pwd, "getpwnam", return_value=account):
                for path, message in (
                    (link, "unsafe"),
                    (oversize, "unsafe"),
                    (outside, "escaped"),
                ):
                    with (
                        self.subTest(path=path),
                        self.assertRaisesRegex(helper.HelperError, message),
                    ):
                        helper.open_agent_stdin(root, path)

    def test_launch_stdin_rejects_identity_change_during_open(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            brief = root / "brief.md"
            brief.write_text("brief", encoding="utf-8")
            actual = brief.stat()
            changed = types.SimpleNamespace(
                st_mode=actual.st_mode,
                st_uid=actual.st_uid,
                st_size=actual.st_size,
                st_dev=actual.st_dev,
                st_ino=actual.st_ino + 1,
            )
            account = types.SimpleNamespace(pw_uid=os.getuid())
            with (
                mock.patch.object(helper.pwd, "getpwnam", return_value=account),
                mock.patch.object(helper.os, "fstat", return_value=changed),
                self.assertRaisesRegex(helper.HelperError, "changed during open"),
            ):
                helper.open_agent_stdin(root, brief)

    def test_launch_envelope_without_stdin_keeps_devnull_default(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "agent"
            executable.write_text("#!/bin/sh\n", encoding="utf-8")
            (root / "workspace").mkdir()
            envelope = root / "harness/launch.json"
            envelope.parent.mkdir()
            envelope.write_text(
                json.dumps(
                    {
                        "argv": [str(executable)],
                        "cwd": str(root / "workspace"),
                        "environment": {},
                    }
                ),
                encoding="utf-8",
            )
            envelope.chmod(0o440)
            real_lstat = Path.lstat

            def protected_lstat(path, *args, **kwargs):
                status = real_lstat(path, *args, **kwargs)
                if path == envelope:
                    return types.SimpleNamespace(st_mode=status.st_mode, st_uid=0)
                return status

            with mock.patch.object(Path, "lstat", protected_lstat):
                (
                    _argv,
                    _environment,
                    _cwd,
                    stdin_path,
                    credential_names,
                    executable_sha256,
                    support_executables,
                ) = helper.load_launch(root)
            self.assertIsNone(stdin_path)
            self.assertEqual(credential_names, ())
            self.assertIsNone(executable_sha256)
            self.assertEqual(support_executables, ())
            self.assertIsNone(helper.open_agent_stdin(root, stdin_path))
            self.assertIn("else subprocess.DEVNULL", HELPER_PATH.read_text(encoding="utf-8"))

    def test_production_launch_pins_executable_and_credential_policy(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "attempt"
            executable_root = Path(directory) / "bin"
            executable_root.mkdir()
            executable = executable_root / "codex"
            executable.write_text("#!/bin/sh\n", encoding="utf-8")
            executable.chmod(0o555)
            workspace = root / "workspace"
            workspace.mkdir(parents=True)
            brief = root / "task/brief.md"
            brief.parent.mkdir()
            brief.write_text("brief\n", encoding="utf-8")
            envelope = root / "harness/launch.json"
            envelope.parent.mkdir()
            envelope.write_text(
                json.dumps(
                    {
                        "argv": [str(executable), "exec", "-"],
                        "cwd": str(workspace),
                        "environment": {
                            "HOME": str(root / "home"),
                            "HTTPS_PROXY": "http://192.0.2.10:8443",
                            "NO_PROXY": "localhost,127.0.0.1",
                            "https_proxy": "http://192.0.2.10:8443",
                            "no_proxy": "localhost,127.0.0.1",
                        },
                        "stdin_path": str(brief),
                        "harness_kind": "codex",
                        "executable_sha256": helper.sha256(executable),
                        "credential_names": ["OPENAI_API_KEY"],
                        "support_executables": [
                            {
                                "path": "/usr/local/bin/codex-code-mode-host",
                                "sha256": "b" * 64,
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            envelope.chmod(0o440)
            real_lstat = Path.lstat

            def protected_lstat(path, *args, **kwargs):
                status = real_lstat(path, *args, **kwargs)
                if path == envelope:
                    return types.SimpleNamespace(st_mode=status.st_mode, st_uid=0)
                if path == executable:
                    return types.SimpleNamespace(
                        st_mode=status.st_mode,
                        st_uid=0,
                        st_gid=0,
                    )
                return status

            with (
                mock.patch.object(helper, "PRODUCTION_EXECUTABLE_ROOTS", (executable_root,)),
                mock.patch.object(Path, "lstat", protected_lstat),
                mock.patch.object(helper, "_root_owned_unwritable"),
                mock.patch.object(helper, "validate_production_executable") as validate_executable,
            ):
                (
                    argv,
                    environment,
                    cwd,
                    stdin_path,
                    credential_names,
                    executable_sha256,
                    support_executables,
                ) = helper.load_launch(root)
            self.assertEqual(argv[0], str(executable))
            self.assertEqual(
                environment,
                {
                    "HOME": str(root / "home"),
                    "HTTPS_PROXY": "http://192.0.2.10:8443",
                    "NO_PROXY": "localhost,127.0.0.1",
                    "https_proxy": "http://192.0.2.10:8443",
                    "no_proxy": "localhost,127.0.0.1",
                },
            )
            self.assertEqual(cwd, workspace)
            self.assertEqual(stdin_path, brief)
            self.assertEqual(credential_names, ("OPENAI_API_KEY",))
            self.assertEqual(executable_sha256, helper.sha256(executable))
            self.assertEqual(
                support_executables,
                ((Path("/usr/local/bin/codex-code-mode-host"), "b" * 64),),
            )
            validate_executable.assert_has_calls(
                [
                    mock.call(executable, helper.sha256(executable)),
                    mock.call(
                        Path("/usr/local/bin/codex-code-mode-host"),
                        "b" * 64,
                        expected_mode=0o755,
                    ),
                ]
            )

            document = json.loads(envelope.read_text(encoding="utf-8"))
            document["harness_kind"] = "opencode"
            document["credential_names"] = []
            document.pop("support_executables")
            envelope.chmod(0o640)
            envelope.write_text(json.dumps(document), encoding="utf-8")
            envelope.chmod(0o440)
            with (
                mock.patch.object(helper, "PRODUCTION_EXECUTABLE_ROOTS", (executable_root,)),
                mock.patch.object(Path, "lstat", protected_lstat),
                mock.patch.object(helper, "_root_owned_unwritable"),
                mock.patch.object(helper, "validate_production_executable"),
            ):
                loaded = helper.load_launch(root)
            self.assertEqual(loaded[4], ())

            document["environment"]["http_proxy"] = "http://192.0.2.10:8443"
            envelope.chmod(0o640)
            envelope.write_text(json.dumps(document), encoding="utf-8")
            envelope.chmod(0o440)
            with (
                mock.patch.object(helper, "PRODUCTION_EXECUTABLE_ROOTS", (executable_root,)),
                mock.patch.object(Path, "lstat", protected_lstat),
                mock.patch.object(helper, "_root_owned_unwritable"),
                self.assertRaisesRegex(helper.HelperError, "environment is invalid"),
            ):
                helper.load_launch(root)

    def test_production_executable_rejects_writable_parent_and_path_swap(self) -> None:
        executable = Path("/usr/local/bin/codex")
        protected = types.SimpleNamespace(st_mode=stat.S_IFREG | 0o555, st_uid=0, st_gid=0)
        with (
            mock.patch.object(helper.Path, "lstat", return_value=protected),
            mock.patch.object(helper, "sha256", return_value="a" * 64),
            mock.patch.object(helper, "PRODUCTION_EXECUTABLE_ROOTS", (Path("/usr/local/bin"),)),
            mock.patch.object(
                helper,
                "_root_owned_unwritable",
                side_effect=helper.HelperError("production executable path is not root protected"),
            ),
            self.assertRaisesRegex(helper.HelperError, "root protected"),
        ):
            helper.validate_production_executable(executable, "a" * 64)

        with (
            mock.patch.object(
                helper,
                "validate_production_executable",
                side_effect=helper.HelperError("production executable identity mismatch"),
            ),
            self.assertRaisesRegex(helper.HelperError, "identity mismatch"),
        ):
            helper.run_agent_process(
                argv=["/usr/local/bin/codex"],
                cwd=Path("/"),
                stdin=helper.subprocess.DEVNULL,
                stdout=io.BytesIO(),
                stderr=helper.subprocess.DEVNULL,
                environment={},
                drop_to_agent=lambda: None,
                codex_auth_file=None,
                codex_auth_content=None,
                agent_account=types.SimpleNamespace(),
                production_executable_sha256="a" * 64,
            )

    def test_credential_envelope_is_exact_bounded_and_duplicate_free(self) -> None:
        expected = ("OPENAI_API_KEY",)
        self.assertEqual(
            helper.parse_credential_envelope(b'{"OPENAI_API_KEY":"synthetic-secret"}', expected),
            {"OPENAI_API_KEY": "synthetic-secret"},
        )
        invalid = (
            b"",
            b"{}",
            b'{"ANTHROPIC_API_KEY":"x"}',
            b'{"OPENAI_API_KEY":""}',
            b'{"OPENAI_API_KEY":"a","OPENAI_API_KEY":"b"}',
            b'{"OPENAI_API_KEY":1}',
            b'{"OPENAI_API_KEY":"nul\\u0000value"}',
        )
        for content in invalid:
            with self.subTest(content=content), self.assertRaises(helper.HelperError):
                helper.parse_credential_envelope(content, expected)
        oversized = json.dumps(
            {"OPENAI_API_KEY": "x" * (helper.MAX_CREDENTIAL_VALUE_BYTES + 1)}
        ).encode("utf-8")
        with self.assertRaisesRegex(helper.HelperError, "too large"):
            helper.parse_credential_envelope(oversized, expected)

    def test_codex_auth_bundle_is_strict_and_non_refreshable(self) -> None:
        bundle = json.dumps(
            {
                "auth_mode": "chatgpt",
                "tokens": {
                    "id_token": "a.b.c",
                    "access_token": "d.e.f",
                    "refresh_token": "cdb-non-refreshable",
                    "account_id": "acct_123",
                },
                "last_refresh": "2026-08-17T12:00:00.000000Z",
            },
            separators=(",", ":"),
        )
        self.assertEqual(helper.parse_codex_auth_json(bundle), bundle.encode())
        invalid = (
            bundle.replace("cdb-non-refreshable", "reusable-refresh-token"),
            bundle.replace('"auth_mode":"chatgpt",', ""),
            bundle.replace('"account_id":"acct_123"', '"unexpected":"x"'),
            bundle.replace('"a.b.c"', '"not-a-jwt"'),
            bundle.replace("2026-08-17T12:00:00.000000Z", "2200-01-01T00:00:00Z"),
        )
        for value in invalid:
            with self.subTest(value=value), self.assertRaises(helper.HelperError):
                helper.parse_codex_auth_json(value)

    def test_codex_auth_policy_is_codex_only_and_never_child_environment(self) -> None:
        self.assertIn("CDB_CODEX_AUTH_JSON", helper.PRODUCTION_CREDENTIALS["codex"])
        self.assertNotIn("CDB_CODEX_AUTH_JSON", helper.PRODUCTION_CREDENTIALS["claude-code"])
        self.assertNotIn("CDB_CODEX_AUTH_JSON", helper.PRODUCTION_CREDENTIALS["opencode"])
        credentials = {"CDB_CODEX_AUTH_JSON": "secret", "OPENAI_API_KEY": "key"}
        auth = credentials.pop("CDB_CODEX_AUTH_JSON")
        child_environment = {"HOME": "/attempt/home", **credentials}
        self.assertNotIn("CDB_CODEX_AUTH_JSON", child_environment)
        self.assertEqual(auth, "secret")

    def test_codex_auth_file_is_truncated_and_removed_after_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            auth_path = Path(directory) / "auth.json"
            account = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
            expected_status = types.SimpleNamespace(
                st_mode=stat.S_IFREG | 0o440,
                st_uid=0,
                st_gid=account.pw_gid,
                st_dev=1,
                st_ino=2,
                st_nlink=1,
            )
            with (
                mock.patch.object(helper.os, "fchown") as fchown,
                mock.patch.object(helper.os, "fstat", return_value=expected_status),
            ):
                descriptor, parent_descriptor, _identity = helper.write_codex_auth(
                    auth_path, b"new-secret", account
                )
            self.assertEqual(auth_path.read_bytes(), b"new-secret")
            self.assertEqual(stat.S_IMODE(auth_path.stat().st_mode), 0o440)
            fchown.assert_called_once_with(mock.ANY, 0, account.pw_gid)
            os.close(descriptor)
            os.close(parent_descriptor)
            auth_path.chmod(0o600)  # Unit tests are not root; production helper is.
            auth_path.unlink()

    def test_codex_auth_refuses_to_overwrite_an_existing_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            auth_path = Path(directory) / "auth.json"
            auth_path.write_bytes(b"preexisting")
            account = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
            with self.assertRaisesRegex(helper.HelperError, "cannot be written"):
                helper.write_codex_auth(auth_path, b"new-secret", account)
            self.assertEqual(auth_path.read_bytes(), b"preexisting")

    def test_codex_auth_lifecycle_removes_file_when_child_errors(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            auth_path = Path(directory) / "auth.json"
            account = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
            child_environment = {"HOME": directory}

            def stage(path, content, _account):
                parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
                descriptor = os.open(
                    path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=parent
                )
                os.write(descriptor, content)
                status = os.fstat(descriptor)
                return descriptor, parent, (status.st_dev, status.st_ino, status.st_nlink)

            with (
                mock.patch.object(helper, "write_codex_auth", side_effect=stage),
                mock.patch.object(
                    helper.subprocess, "Popen", side_effect=OSError("child failed")
                ) as popen,
                self.assertRaisesRegex(OSError, "child failed"),
            ):
                helper.run_agent_process(
                    argv=["/usr/bin/false"],
                    cwd=Path(directory),
                    stdin=helper.subprocess.DEVNULL,
                    stdout=io.BytesIO(),
                    stderr=helper.subprocess.DEVNULL,
                    environment=child_environment,
                    drop_to_agent=lambda: None,
                    codex_auth_file=auth_path,
                    codex_auth_content=b"ephemeral-secret",
                    agent_account=account,
                )
            self.assertFalse(auth_path.exists())
            self.assertNotIn("CDB_CODEX_AUTH_JSON", popen.call_args.kwargs["env"])

    def test_codex_auth_is_revoked_after_startup_before_more_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            auth_path = Path(directory) / "auth.json"
            script = (
                "import json,pathlib,sys,time; p=pathlib.Path(sys.argv[1]); "
                "print(json.dumps({'type':'thread.started','auth_at_startup':p.exists()}),flush=True); "
                "deadline=time.monotonic()+2; "
                'exec("while p.exists() and time.monotonic() < deadline:\\n time.sleep(.01)"); '
                "print(json.dumps({'auth_after_startup':p.exists()}),flush=True)"
            )
            output = io.BytesIO()

            def stage(path, content, _account):
                parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
                descriptor = os.open(
                    path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=parent
                )
                os.write(descriptor, content)
                status = os.fstat(descriptor)
                return descriptor, parent, (status.st_dev, status.st_ino, status.st_nlink)

            with mock.patch.object(helper, "write_codex_auth", side_effect=stage):
                helper.run_agent_process(
                    argv=["/usr/bin/python3", "-c", script, str(auth_path)],
                    cwd=Path(directory),
                    stdin=helper.subprocess.DEVNULL,
                    stdout=output,
                    stderr=helper.subprocess.DEVNULL,
                    environment={},
                    drop_to_agent=lambda: None,
                    codex_auth_file=auth_path,
                    codex_auth_content=b"ephemeral-secret",
                    agent_account=types.SimpleNamespace(),
                )
            self.assertFalse(auth_path.exists())
            self.assertEqual(
                [json.loads(line) for line in output.getvalue().splitlines()],
                [
                    {"type": "thread.started", "auth_at_startup": True},
                    {"auth_after_startup": False},
                ],
            )

    def test_codex_auth_path_swap_hardlink_and_missing_startup_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            auth_path = base / "auth.json"

            def stage(path, content, _account):
                parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
                descriptor = os.open(
                    path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=parent
                )
                os.write(descriptor, content)
                status = os.fstat(descriptor)
                return descriptor, parent, (status.st_dev, status.st_ino, status.st_nlink)

            cases = {
                "rename": "p=pathlib.Path(sys.argv[1]); p.replace(p.with_name('old')); p.write_text('replacement'); print('{\\\"type\\\":\\\"thread.started\\\"}',flush=True); time.sleep(.2)",
                "swap": "p=pathlib.Path(sys.argv[1]); os.link(p,p.with_name('original')); q=p.with_name('replacement'); q.write_text('swap'); q.replace(p); print('{\\\"type\\\":\\\"thread.started\\\"}',flush=True); time.sleep(.2)",
                "hardlink": "os.link(sys.argv[1], sys.argv[1]+'-link'); print('{\\\"type\\\":\\\"thread.started\\\"}',flush=True); time.sleep(.2)",
                "silent": "time.sleep(1)",
            }
            for name, body in cases.items():
                with self.subTest(name=name):
                    for path in base.iterdir():
                        if path.is_file():
                            path.unlink()
                    script = "import os,pathlib,sys,time; " + body
                    with (
                        mock.patch.object(helper, "write_codex_auth", side_effect=stage),
                        mock.patch.object(helper, "CODEX_STARTUP_TIMEOUT_SECONDS", 0.5),
                        self.assertRaises(helper.HelperError),
                    ):
                        helper.run_agent_process(
                            argv=["/usr/bin/python3", "-c", script, str(auth_path)],
                            cwd=base,
                            stdin=helper.subprocess.DEVNULL,
                            stdout=io.BytesIO(),
                            stderr=helper.subprocess.DEVNULL,
                            environment={},
                            drop_to_agent=lambda: None,
                            codex_auth_file=auth_path,
                            codex_auth_content=b"ephemeral-secret",
                            agent_account=types.SimpleNamespace(),
                        )
                    if name == "rename":
                        self.assertEqual(auth_path.read_text(), "replacement")
                        self.assertEqual((base / "old").read_bytes(), b"")
                    elif name == "swap":
                        self.assertEqual(auth_path.read_text(), "swap")
                        self.assertEqual((base / "original").read_bytes(), b"")
                    elif name == "hardlink":
                        self.assertEqual((base / "auth.json-link").read_bytes(), b"")
                    else:
                        self.assertFalse(auth_path.exists())

    @staticmethod
    def _claude_startup(**overrides):
        event = {
            "type": "system",
            "subtype": "init",
            "claude_code_version": "2.1.233",
            "model": "claude-sonnet-4-5",
            "permissionMode": "bypassPermissions",
            "session_id": "synthetic-session",
            "uuid": "synthetic-event",
            "tools": [],
            "mcp_servers": [],
        }
        event.update(overrides)
        return event

    @staticmethod
    def _claude_rate_limit(**overrides):
        event = {
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed",
                "rateLimitType": "five_hour",
                "utilization": 0.25,
            },
            "uuid": "synthetic-rate-limit-event",
            "session_id": "synthetic-session",
        }
        event.update(overrides)
        return event

    def test_claude_oauth_fd_is_one_shot_secret_free_and_exhausted_for_descendants(self) -> None:
        secret = b"synthetic-claude-oauth-secret"
        secret_sha256 = hashlib.sha256(secret).hexdigest()
        token = bytearray(secret)
        startup = json.dumps(self._claude_startup(), separators=(",", ":"))
        script = (
            "import json,os,subprocess,sys; "
            "fd=int(os.environ['CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR']); "
            "token=os.read(fd,65536); "
            f"print({startup!r},flush=True); "
            f'code=\'import hashlib,json,os; expected=\\"{secret_sha256}\\"; fd=int(os.environ.get(\\"CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR\\",\\"-1\\")); '
            'data=os.read(fd,1); status=\\"eof\\" if not data else \\"bytes\\"; '
            'print(json.dumps({\\"env_secret\\":any(hashlib.sha256(v.encode()).hexdigest()==expected for v in os.environ.values()),\\"fd_status\\":status,\\"fd_bytes\\":len(data)}))\'; '
            "result=subprocess.run([sys.executable,'-c',code],stdout=subprocess.PIPE,text=True,close_fds=False); "
            "files=any(token in p.read_bytes() for p in __import__('pathlib').Path('.').rglob('*') if p.is_file()); "
            "print(json.dumps({'argv_secret':any(token.decode() in a for a in sys.argv),'env_secret':any(token.decode() in v for v in os.environ.values()),'file_secret':files,'descendant':json.loads(result.stdout)}),flush=True)"
        )
        output = io.BytesIO()
        with tempfile.TemporaryDirectory() as directory:
            completed = helper.run_claude_with_oauth_fd(
                argv=["/usr/bin/python3", "-c", script, "--model", "claude-sonnet-4-5"],
                cwd=Path(directory),
                stdin=helper.subprocess.DEVNULL,
                stdout=output,
                stderr=helper.subprocess.DEVNULL,
                environment={"HOME": directory},
                drop_to_agent=lambda: None,
                token=token,
            )
        self.assertEqual(completed.returncode, 0)
        self.assertEqual(token, bytearray(len(secret)))
        events = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual(events[0], self._claude_startup())
        self.assertEqual(
            events[1],
            {
                "argv_secret": False,
                "env_secret": False,
                "file_secret": False,
                "descendant": {"env_secret": False, "fd_status": "eof", "fd_bytes": 0},
            },
        )

    def test_claude_rate_limit_prelude_is_accepted_and_preserved_exactly(self) -> None:
        records = [
            self._claude_rate_limit(),
            self._claude_rate_limit(uuid="synthetic-rate-limit-event-2"),
            self._claude_startup(),
            {"type": "assistant", "message": "after-init"},
        ]
        payload = b"".join(
            json.dumps(record, separators=(",", ":")).encode() + b"\n" for record in records
        )
        script = (
            "import os,sys; "
            "fd=int(os.environ['CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR']); "
            "os.read(fd,65536); "
            f"sys.stdout.buffer.write({payload!r}); sys.stdout.buffer.flush()"
        )
        output = io.BytesIO()
        token = bytearray(b"synthetic-secret")
        with tempfile.TemporaryDirectory() as directory:
            completed = helper.run_claude_with_oauth_fd(
                argv=["/usr/bin/python3", "-c", script, "--model", "claude-sonnet-4-5"],
                cwd=Path(directory),
                stdin=helper.subprocess.DEVNULL,
                stdout=output,
                stderr=helper.subprocess.DEVNULL,
                environment={},
                drop_to_agent=lambda: None,
                token=token,
            )
        self.assertEqual(completed.returncode, 0)
        self.assertEqual(output.getvalue(), payload)
        self.assertEqual(token, bytearray(len(b"synthetic-secret")))

    def test_claude_oauth_fd_malformed_missing_and_unconsumed_startup_fail_closed(self) -> None:
        valid = json.dumps(self._claude_startup(), separators=(",", ":"))
        rate_limit = json.dumps(self._claude_rate_limit(), separators=(",", ":"))
        too_many = "\n".join(
            [rate_limit] * (helper.MAX_CLAUDE_STARTUP_PRELUDE_EVENTS + 1) + [valid]
        )
        cases = {
            "malformed": "os.read(fd,65536); print('{bad json',flush=True)",
            "unexpected-prelude": 'os.read(fd,65536); print(\'{"type":"assistant"}\',flush=True)',
            "wrong-version": f"os.read(fd,65536); print({json.dumps(json.dumps(self._claude_startup(claude_code_version='2.1.234')))},flush=True)",
            "unconsumed": f"print({json.dumps(valid)},flush=True); time.sleep(1)",
            "missing": "os.read(fd,65536); time.sleep(1)",
            "prelude-eof": f"os.read(fd,65536); print({rate_limit!r},flush=True)",
            "too-many-prelude-events": f"os.read(fd,65536); print({too_many!r},flush=True)",
            "oversized-prelude": (
                f"os.read(fd,65536); os.write(1,b'x'*{helper.MAX_CLAUDE_STARTUP_LINE_BYTES + 1})"
            ),
        }
        for name, body in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                token = bytearray(b"synthetic-secret")
                output = io.BytesIO()
                script = (
                    "import os,time; fd=int(os.environ['CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR']); "
                    + body
                )
                with (
                    mock.patch.object(helper, "CLAUDE_STARTUP_TIMEOUT_SECONDS", 0.2),
                    self.assertRaises(helper.HelperError),
                ):
                    helper.run_claude_with_oauth_fd(
                        argv=["/usr/bin/python3", "-c", script, "--model", "claude-sonnet-4-5"],
                        cwd=Path(directory),
                        stdin=helper.subprocess.DEVNULL,
                        stdout=output,
                        stderr=helper.subprocess.DEVNULL,
                        environment={},
                        drop_to_agent=lambda: None,
                        token=token,
                    )
                self.assertEqual(token, bytearray(len(b"synthetic-secret")))
                self.assertEqual(output.getvalue(), b"")

    def test_claude_rate_limit_prelude_uses_one_overall_startup_timeout(self) -> None:
        rate_limit = json.dumps(self._claude_rate_limit(), separators=(",", ":"))
        startup = json.dumps(self._claude_startup(), separators=(",", ":"))
        script = (
            "import os,time; "
            "fd=int(os.environ['CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR']); "
            "os.read(fd,65536); "
            f"print({rate_limit!r},flush=True); time.sleep(.2); "
            f"print({rate_limit!r},flush=True); time.sleep(.2); "
            f"print({startup!r},flush=True)"
        )
        token = bytearray(b"synthetic-secret")
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(helper, "CLAUDE_STARTUP_TIMEOUT_SECONDS", 0.3),
            self.assertRaisesRegex(helper.HelperError, "did not emit"),
        ):
            helper.run_claude_with_oauth_fd(
                argv=["/usr/bin/python3", "-c", script, "--model", "claude-sonnet-4-5"],
                cwd=Path(directory),
                stdin=helper.subprocess.DEVNULL,
                stdout=io.BytesIO(),
                stderr=helper.subprocess.DEVNULL,
                environment={},
                drop_to_agent=lambda: None,
                token=token,
            )
        self.assertEqual(token, bytearray(len(b"synthetic-secret")))

    def test_claude_oauth_policy_rejects_wrong_name_harness_and_reserved_fd(self) -> None:
        self.assertEqual(
            helper.PRODUCTION_CREDENTIALS["claude-code"],
            {"CLAUDE_CODE_OAUTH_TOKEN"},
        )
        self.assertNotIn("CLAUDE_CODE_OAUTH_TOKEN", helper.PRODUCTION_CREDENTIALS["opencode"])
        with self.assertRaisesRegex(helper.HelperError, "does not match"):
            helper.parse_credential_envelope(
                b'{"ANTHROPIC_API_KEY":"synthetic"}',
                ("CLAUDE_CODE_OAUTH_TOKEN",),
            )

        oversized = bytearray(b"x" * (helper.MAX_CLAUDE_OAUTH_TOKEN_BYTES + 1))
        with (
            mock.patch.object(helper.os, "pipe") as pipe,
            self.assertRaisesRegex(helper.HelperError, "token is invalid"),
        ):
            helper.run_claude_with_oauth_fd(
                argv=["/usr/bin/false", "--model", "model"],
                cwd=Path("/"),
                stdin=helper.subprocess.DEVNULL,
                stdout=io.BytesIO(),
                stderr=helper.subprocess.DEVNULL,
                environment={},
                drop_to_agent=lambda: None,
                token=oversized,
            )
        pipe.assert_not_called()
        self.assertEqual(oversized, bytearray(len(oversized)))

        token = bytearray(b"synthetic-secret")
        original = os.open(os.devnull, os.O_RDONLY)
        try:
            os.dup2(original, helper.CLAUDE_CODE_OAUTH_FD)
            identity = os.fstat(helper.CLAUDE_CODE_OAUTH_FD)
            with self.assertRaisesRegex(helper.HelperError, "already in use"):
                helper.run_claude_with_oauth_fd(
                    argv=["/usr/bin/false", "--model", "model"],
                    cwd=Path("/"),
                    stdin=helper.subprocess.DEVNULL,
                    stdout=io.BytesIO(),
                    stderr=helper.subprocess.DEVNULL,
                    environment={},
                    drop_to_agent=lambda: None,
                    token=token,
                )
            self.assertEqual(os.fstat(helper.CLAUDE_CODE_OAUTH_FD), identity)
            self.assertEqual(token, bytearray(len(b"synthetic-secret")))
        finally:
            os.close(helper.CLAUDE_CODE_OAUTH_FD)
            os.close(original)

    def test_credential_launch_is_explicit_and_one_shot_dispatch(self) -> None:
        with (
            mock.patch.object(helper.os, "geteuid", return_value=0),
            mock.patch.dict(os.environ, {"SUDO_USER": "lume"}, clear=False),
            mock.patch.object(helper, "launch_agent", return_value={"ok": True}) as launch,
            mock.patch.object(helper, "emit"),
        ):
            helper.dispatch(["launch-agent-with-lease", "trial-one"])
        launch.assert_called_once_with("trial-one", credential_input=True)

    def test_launch_stage_diagnostic_is_fixed_and_suppresses_cause(self) -> None:
        secret = "sk-test-secret-never-reported"
        with (
            mock.patch.object(helper, "load_launch", side_effect=RuntimeError(secret)),
            self.assertRaises(helper.LaunchStageError) as raised,
        ):
            helper.launch_agent("trial-one", credential_input=True)
        self.assertEqual(str(raised.exception), "launch-stage-envelope-load")
        self.assertNotIn(secret, str(raised.exception))
        self.assertIn("launch-stage-envelope-load", helper.LAUNCH_STAGES)

    def test_production_attempt_uses_bounded_stable_pf_anchor(self) -> None:
        attempt = "trial-syn-t01-system-codex-production-gate-normal-seed-610001"
        self.assertEqual(len(attempt), 61)
        anchor = helper.pf_anchor(attempt)
        expected = "com.trycua.cdb/a-" + hashlib.sha256(attempt.encode("ascii")).hexdigest()[:40]
        self.assertEqual(anchor, expected)
        self.assertEqual(helper.pf_anchor(attempt), anchor)
        self.assertNotEqual(helper.pf_anchor(attempt + "-x"), anchor)
        self.assertEqual(len(anchor.encode("ascii")), 57)
        self.assertLessEqual(len(anchor.encode("ascii")), helper.MAX_PF_ANCHOR_NAME_BYTES)

        main_rules = 'anchor "com.trycua.cdb/*"\n'
        anchor_rules = "block drop out quick all\n"
        responses = [
            types.SimpleNamespace(stdout=main_rules, stderr=""),
            types.SimpleNamespace(stdout="", stderr=""),
            types.SimpleNamespace(stdout="Token : 42\n", stderr=""),
            types.SimpleNamespace(stdout="Status: Enabled\n", stderr=""),
            types.SimpleNamespace(stdout=anchor_rules, stderr=""),
        ]
        account = types.SimpleNamespace(pw_uid=502)
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(helper, "STATE_BASE", Path(directory)),
            mock.patch.object(helper.pwd, "getpwnam", return_value=account),
            mock.patch.object(helper, "run_pf", side_effect=responses) as run_pf,
        ):
            evidence = helper.apply_network(
                attempt,
                "none",
                hashlib.sha256(main_rules.encode()).hexdigest(),
                [],
            )
        self.assertEqual(evidence["anchor"], anchor)
        self.assertIn(
            mock.call("anchor-load", ["-a", anchor, "-f", "-"], input_text=mock.ANY),
            run_pf.call_args_list,
        )
        self.assertIn(
            mock.call("anchor-rules-read", ["-a", anchor, "-sr"]),
            run_pf.call_args_list,
        )

    def test_network_cleanup_is_scoped_to_recorded_anchor(self) -> None:
        anchor = helper.pf_anchor("trial-one")
        with tempfile.TemporaryDirectory() as directory:
            state_root = Path(directory)
            state = state_root / "trial-one.json"
            state.write_text(
                json.dumps(
                    {
                        "attempt": "trial-one",
                        "network": {
                            "anchor": anchor,
                            "pf_token": "42",
                        },
                    }
                ),
                encoding="utf-8",
            )
            with (
                mock.patch.object(helper, "STATE_BASE", state_root),
                mock.patch.object(helper, "run_pf") as run_pf,
            ):
                evidence = helper.remove_network("trial-one")
            self.assertTrue(evidence["removed"])
            self.assertEqual(
                [call.args for call in run_pf.call_args_list],
                [
                    (
                        "anchor-flush",
                        ["-a", anchor, "-F", "rules"],
                    ),
                    ("pf-token-release", ["-X", "42"]),
                ],
            )
            self.assertEqual(json.loads(state.read_text()), {"attempt": "trial-one"})

    def test_network_cleanup_rejects_legacy_unbounded_anchor(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            state_root = Path(directory)
            (state_root / "trial-one.json").write_text(
                json.dumps(
                    {
                        "attempt": "trial-one",
                        "network": {
                            "anchor": "com.trycua.cdb/trial-one",
                            "pf_token": "42",
                        },
                    }
                ),
                encoding="utf-8",
            )
            with (
                mock.patch.object(helper, "STATE_BASE", state_root),
                mock.patch.object(helper, "run_pf") as run_pf,
                self.assertRaisesRegex(helper.HelperError, "malformed"),
            ):
                helper.remove_network("trial-one")
            run_pf.assert_not_called()

    def test_network_status_reprobes_without_rewriting_rules(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            state_root = Path(directory)
            main_rules = 'anchor "com.trycua.cdb/*"\n'
            anchor_rules = "block drop out quick all\n"
            network = {
                "anchor": helper.pf_anchor("trial-one"),
                "mode": "none",
                "rules_sha256": "1" * 64,
                "ruleset_main_sha256": hashlib.sha256(main_rules.encode()).hexdigest(),
                "anchor_rules_sha256": hashlib.sha256(anchor_rules.encode()).hexdigest(),
                "pf_enabled": True,
                "allowlist_sha256": hashlib.sha256(b"[]").hexdigest(),
                "residual_channels": [
                    "unix-domain-sockets",
                    "shared-filesystem",
                    "console-user-gui-egress",
                    "deferred-scheduling",
                ],
                "global_protocol_blocks": ["dns", "mdns", "icmp", "ipv6-icmp"],
                "pf_token": "42",
            }
            (state_root / "trial-one.json").write_text(
                json.dumps({"attempt": "trial-one", "network": network}),
                encoding="utf-8",
            )
            responses = [
                types.SimpleNamespace(stdout=main_rules, stderr=""),
                types.SimpleNamespace(stdout=anchor_rules, stderr=""),
                types.SimpleNamespace(stdout="Status: Enabled\n", stderr=""),
            ]
            with (
                mock.patch.object(helper, "STATE_BASE", state_root),
                mock.patch.object(helper, "run_pf", side_effect=responses) as run_pf,
            ):
                evidence = helper.network_status("trial-one")
            self.assertEqual(evidence["verb"], "network-status")
            self.assertNotIn("pf_token", evidence)
            self.assertEqual(
                [call.args for call in run_pf.call_args_list],
                [
                    ("main-rules-read", ["-sr"]),
                    ("anchor-rules-read", ["-a", network["anchor"], "-sr"]),
                    ("pf-info-read", ["-s", "info"]),
                ],
            )

    def test_network_status_fails_when_anchor_changes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            state_root = Path(directory)
            main_rules = 'anchor "com.trycua.cdb/*"\n'
            network = {
                "anchor": helper.pf_anchor("trial-one"),
                "mode": "none",
                "rules_sha256": "1" * 64,
                "ruleset_main_sha256": hashlib.sha256(main_rules.encode()).hexdigest(),
                "anchor_rules_sha256": "2" * 64,
                "pf_enabled": True,
                "allowlist_sha256": hashlib.sha256(b"[]").hexdigest(),
                "residual_channels": [],
                "global_protocol_blocks": [],
                "pf_token": "42",
            }
            (state_root / "trial-one.json").write_text(
                json.dumps({"attempt": "trial-one", "network": network}),
                encoding="utf-8",
            )
            responses = [
                types.SimpleNamespace(stdout=main_rules, stderr=""),
                types.SimpleNamespace(stdout="changed\n", stderr=""),
                types.SimpleNamespace(stdout="Status: Enabled\n", stderr=""),
            ]
            with (
                mock.patch.object(helper, "STATE_BASE", state_root),
                mock.patch.object(helper, "run_pf", side_effect=responses),
                self.assertRaisesRegex(helper.HelperError, "changed"),
            ):
                helper.network_status("trial-one")

    def test_network_apply_failure_rolls_back_anchor_and_pf_token(self) -> None:
        anchor = helper.pf_anchor("trial-one")
        with tempfile.TemporaryDirectory() as directory:
            state_root = Path(directory)
            responses = [
                types.SimpleNamespace(stdout='anchor "com.trycua.cdb/*"\n', stderr=""),
                types.SimpleNamespace(stdout="", stderr=""),
                types.SimpleNamespace(stdout="", stderr="Token : 42\n"),
                types.SimpleNamespace(stdout="Status: Disabled\n", stderr=""),
                types.SimpleNamespace(stdout="rules\n", stderr=""),
                types.SimpleNamespace(stdout="", stderr=""),
                types.SimpleNamespace(stdout="", stderr=""),
            ]
            account = types.SimpleNamespace(pw_uid=502)
            with (
                mock.patch.object(helper, "STATE_BASE", state_root),
                mock.patch.object(helper.pwd, "getpwnam", return_value=account),
                mock.patch.object(helper, "run_pf", side_effect=responses) as run_pf,
                self.assertRaisesRegex(helper.HelperError, "could not be verified"),
            ):
                helper.apply_network(
                    "trial-one",
                    "none",
                    hashlib.sha256('anchor "com.trycua.cdb/*"\n'.encode()).hexdigest(),
                    [],
                )
            commands = [call.args for call in run_pf.call_args_list]
            self.assertIn(
                (
                    "rollback-anchor-flush",
                    ["-a", anchor, "-F", "rules"],
                ),
                commands,
            )
            self.assertIn(("rollback-token-release", ["-X", "42"]), commands)
            self.assertFalse((state_root / "trial-one.json").exists())

    def test_network_apply_anchor_load_failure_reports_fixed_stage_and_exit(self) -> None:
        main_rules = 'anchor "com.trycua.cdb/*"\n'
        responses = [
            types.SimpleNamespace(returncode=0, stdout=main_rules, stderr=""),
            types.SimpleNamespace(
                returncode=23,
                stdout="sensitive argv output",
                stderr="sensitive pf detail",
            ),
            types.SimpleNamespace(returncode=0, stdout="", stderr=""),
        ]
        account = types.SimpleNamespace(pw_uid=502)
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(helper, "STATE_BASE", Path(directory)),
            mock.patch.object(helper.pwd, "getpwnam", return_value=account),
            mock.patch.object(helper.subprocess, "run", side_effect=responses),
            self.assertRaises(helper.HelperError) as raised,
        ):
            helper.apply_network(
                "trial-one",
                "none",
                hashlib.sha256(main_rules.encode()).hexdigest(),
                [],
            )
        self.assertEqual(
            str(raised.exception),
            "PF stage anchor-load failed with exit code 23",
        )
        self.assertNotIn("sensitive", str(raised.exception))

    def test_network_apply_pf_enable_failure_reports_fixed_stage_and_exit(self) -> None:
        main_rules = 'anchor "com.trycua.cdb/*"\n'
        responses = [
            types.SimpleNamespace(returncode=0, stdout=main_rules, stderr=""),
            types.SimpleNamespace(returncode=0, stdout="", stderr=""),
            types.SimpleNamespace(returncode=5, stdout="", stderr="sensitive pf detail"),
            types.SimpleNamespace(returncode=0, stdout="", stderr=""),
        ]
        account = types.SimpleNamespace(pw_uid=502)
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(helper, "STATE_BASE", Path(directory)),
            mock.patch.object(helper.pwd, "getpwnam", return_value=account),
            mock.patch.object(helper.subprocess, "run", side_effect=responses),
            self.assertRaises(helper.HelperError) as raised,
        ):
            helper.apply_network(
                "trial-one",
                "none",
                hashlib.sha256(main_rules.encode()).hexdigest(),
                [],
            )
        self.assertEqual(
            str(raised.exception),
            "PF stage pf-enable failed with exit code 5",
        )
        self.assertNotIn("sensitive", str(raised.exception))

    def test_network_apply_rollback_failure_preserves_primary_failure(self) -> None:
        main_rules = 'anchor "com.trycua.cdb/*"\n'
        responses = [
            types.SimpleNamespace(returncode=0, stdout=main_rules, stderr=""),
            types.SimpleNamespace(returncode=23, stdout="", stderr="primary sensitive detail"),
            types.SimpleNamespace(returncode=9, stdout="", stderr="rollback sensitive detail"),
        ]
        account = types.SimpleNamespace(pw_uid=502)
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(helper, "STATE_BASE", Path(directory)),
            mock.patch.object(helper.pwd, "getpwnam", return_value=account),
            mock.patch.object(helper.subprocess, "run", side_effect=responses),
            self.assertRaises(helper.HelperError) as raised,
        ):
            helper.apply_network(
                "trial-one",
                "none",
                hashlib.sha256(main_rules.encode()).hexdigest(),
                [],
            )
        self.assertEqual(
            str(raised.exception),
            "PF stage anchor-load failed with exit code 23; PF rollback incomplete",
        )
        self.assertNotIn("sensitive", str(raised.exception))
        self.assertNotIn("exit code 9", str(raised.exception))

    def test_kill_terminates_all_agent_uid_processes_and_verifies_none_remain(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            state_root = Path(directory)
            (state_root / "trial-one.json").write_text(
                json.dumps(
                    {
                        "attempt": "trial-one",
                        "agent": {"supervisor_pid": 42, "completed": False},
                    }
                ),
                encoding="utf-8",
            )
            with (
                mock.patch.object(helper, "STATE_BASE", state_root),
                mock.patch.object(
                    helper,
                    "agent_pids",
                    side_effect=[[42, 43], []],
                ),
                mock.patch.object(helper.os, "kill") as kill_process,
            ):
                evidence = helper.kill_agent("trial-one")
            self.assertTrue(evidence["killed"])
            self.assertTrue(evidence["no_agent_processes"])
            self.assertEqual(
                [call.args for call in kill_process.call_args_list],
                [(42, helper.signal.SIGKILL), (43, helper.signal.SIGKILL)],
            )

    def test_supervisor_detaches_standard_streams_and_closes_other_fds(self) -> None:
        read_descriptor, write_descriptor = os.pipe()
        unrelated_read, unrelated_write = os.pipe()
        pid = os.fork()
        if pid == 0:
            try:
                os.close(read_descriptor)
                helper.detach_standard_streams({write_descriptor})
                null_status = os.stat(os.devnull)
                facts = {
                    "stdin": os.fstat(0).st_rdev == null_status.st_rdev,
                    "stdout": os.fstat(1).st_rdev == null_status.st_rdev,
                    "stderr": os.fstat(2).st_rdev == null_status.st_rdev,
                    "unrelated_closed": not Path(f"/dev/fd/{unrelated_write}").exists(),
                }
                os.write(write_descriptor, json.dumps(facts).encode())
            finally:
                os._exit(0)
        os.close(write_descriptor)
        os.close(unrelated_read)
        os.close(unrelated_write)
        payload = os.read(read_descriptor, 4096)
        os.close(read_descriptor)
        os.waitpid(pid, 0)
        facts = json.loads(payload)
        self.assertTrue(facts["stdin"])
        self.assertTrue(facts["stdout"])
        self.assertTrue(facts["stderr"])
        self.assertTrue(facts["unrelated_closed"])

    def test_mediator_child_detaches_ssh_streams_before_exec(self) -> None:
        source = HELPER_PATH.read_text(encoding="utf-8")
        mediator_child = source.split("def start_mediator", 1)[1].split(
            "def _validate_chained_log", 1
        )[0]
        detach = "detach_standard_streams({listener.fileno(), log_fd})"
        self.assertIn(detach, mediator_child)
        self.assertLess(mediator_child.index(detach), mediator_child.index("os.execve"))

    def test_main_pf_ruleset_must_match_frozen_digest(self) -> None:
        main_rules = 'pass out quick all\nanchor "com.trycua.cdb/*"\n'
        account = types.SimpleNamespace(pw_uid=502)
        with (
            mock.patch.object(helper.pwd, "getpwnam", return_value=account),
            mock.patch.object(
                helper,
                "run_pf",
                return_value=types.SimpleNamespace(stdout=main_rules, stderr=""),
            ) as run_pf,
            self.assertRaisesRegex(helper.HelperError, "ruleset digest mismatch"),
        ):
            helper.apply_network("trial-one", "none", "0" * 64, [])
        run_pf.assert_called_once_with("main-rules-read", ["-sr"])

    def test_permissive_quick_rule_before_anchor_is_rejected(self) -> None:
        main_rules = 'pass out quick all\nanchor "com.trycua.cdb/*" all\n'
        account = types.SimpleNamespace(pw_uid=502)
        with (
            mock.patch.object(helper.pwd, "getpwnam", return_value=account),
            mock.patch.object(
                helper,
                "run_pf",
                return_value=types.SimpleNamespace(stdout=main_rules, stderr=""),
            ) as run_pf,
            self.assertRaisesRegex(helper.HelperError, "permissive quick"),
        ):
            helper.apply_network(
                "trial-one", "none", hashlib.sha256(main_rules.encode()).hexdigest(), []
            )
        run_pf.assert_called_once_with("main-rules-read", ["-sr"])

    def test_prior_filter_anchor_is_rejected(self) -> None:
        main_rules = 'anchor "com.apple/*" all\nanchor "com.trycua.cdb/*" all\n'
        account = types.SimpleNamespace(pw_uid=502)
        with (
            mock.patch.object(helper.pwd, "getpwnam", return_value=account),
            mock.patch.object(
                helper,
                "run_pf",
                return_value=types.SimpleNamespace(stdout=main_rules, stderr=""),
            ) as run_pf,
            self.assertRaisesRegex(helper.HelperError, "filter anchor precedes"),
        ):
            helper.apply_network(
                "trial-one", "none", hashlib.sha256(main_rules.encode()).hexdigest(), []
            )
        run_pf.assert_called_once_with("main-rules-read", ["-sr"])

    def test_main_reports_helper_errors_with_contract_exit_code(self) -> None:
        completed = __import__("subprocess").run(
            [str(HELPER_PATH), "unsupported"],
            capture_output=True,
            text=True,
            check=False,
            env={"SUDO_USER": "lume", "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"},
        )
        self.assertEqual(completed.returncode, 64)
        self.assertIn("cdb-helper:", completed.stderr)
        self.assertNotIn("AttributeError", completed.stderr)

    def test_stage_preserves_only_declared_executable_bits(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "agent"
            data = root / "data"
            executable.write_text("#!/bin/sh\n", encoding="utf-8")
            data.write_text("data", encoding="utf-8")
            executable.chmod(0o555)
            data.chmod(0o644)
            account = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
            with (
                mock.patch.object(helper.pwd, "getpwnam", return_value=account),
                mock.patch.object(helper.os, "chown"),
            ):
                for name in ("home", "workspace", "artifacts"):
                    (root / name).mkdir()
                helper.prepare_permissions(root)
            self.assertEqual(stat.S_IMODE(executable.stat().st_mode), 0o550)
            self.assertEqual(stat.S_IMODE(data.stat().st_mode), 0o440)

    def test_console_share_is_narrow_and_rejects_links(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            attempts = Path(directory)
            root = attempts / "trial-one"
            task_store = root / "workspace/task-store"
            task_store.mkdir(parents=True)
            record = task_store / "records.json"
            record.write_text("{}", encoding="utf-8")
            account = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
            group = types.SimpleNamespace(gr_gid=os.getgid())
            with (
                mock.patch.object(helper, "ATTEMPT_BASE", attempts),
                mock.patch.object(helper.pwd, "getpwnam", return_value=account),
                mock.patch.object(helper.grp, "getgrnam", return_value=group),
                mock.patch.object(helper.os, "chown"),
            ):
                evidence = helper.share_console_path("trial-one", "task-store")
                self.assertEqual(evidence["relative"], "task-store")
                self.assertEqual(stat.S_IMODE(record.stat().st_mode), 0o660)
                with self.assertRaisesRegex(helper.HelperError, "unavailable"):
                    helper.share_console_path("trial-one", "seed-exchange")

    def test_task_app_launch_is_closed_and_store_bound(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            attempts = base / "attempts"
            protected = base / "protected"
            root = attempts / "trial-one"
            (root / "task/apps/example-desk").mkdir(parents=True)
            store = protected / "trial-one/task-store"
            store.mkdir(parents=True)
            electron = Path(directory) / "Electron.app"
            binary = electron / "Contents/MacOS/Electron"
            binary.parent.mkdir(parents=True)
            binary.write_text("binary", encoding="utf-8")
            account = types.SimpleNamespace(pw_name="lume")
            calls: list[list[str]] = []

            def fixed_run(argv, **_kwargs):
                calls.append(argv)
                return types.SimpleNamespace(stdout=f"{os.getuid()}\n", stderr="")

            with (
                mock.patch.object(helper, "ATTEMPT_BASE", attempts),
                mock.patch.object(helper, "PROTECTED_BASE", protected),
                mock.patch.object(helper, "STATE_BASE", Path(directory) / "state"),
                mock.patch.object(helper, "ELECTRON_APP", electron),
                mock.patch.object(helper, "run", side_effect=fixed_run),
                mock.patch.object(helper.pwd, "getpwuid", return_value=account),
                mock.patch.object(helper, "_task_app_processes", return_value=[4321]),
                mock.patch.object(helper, "_process_identity", return_value="a" * 64),
                mock.patch.object(helper.time, "sleep"),
            ):
                evidence = helper.launch_task_app(
                    "trial-one",
                    "example-desk",
                    "task-store",
                    '["--record=ITEM-1042"]',
                )
                self.assertEqual(evidence["store_mode"], "protected-console-only")
                self.assertEqual(evidence["target_pid"], 4321)
                with self.assertRaisesRegex(helper.HelperError, "one-shot"):
                    helper.launch_task_app(
                        "trial-one",
                        "example-desk",
                        "task-store",
                        '["--record=ITEM-1042"]',
                    )
            launch = calls[-1]
            self.assertIn("--record=ITEM-1042", launch)
            self.assertIn(f"--store={store}", launch)
            stored = json.loads((base / "state/trial-one.json").read_text())["task_app"]
            self.assertEqual(stored["store_relative"], "task-store")
            self.assertEqual(stored["target_process_identity"], "a" * 64)
            self.assertEqual(stored["store_inode"], store.stat().st_ino)

    def test_task_app_launch_never_falls_back_to_workspace_store(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            attempts = base / "attempts"
            root = attempts / "trial-one"
            (root / "task/apps/example-desk").mkdir(parents=True)
            (root / "workspace/task-store").mkdir(parents=True)
            electron = base / "Electron.app"
            binary = electron / "Contents/MacOS/Electron"
            binary.parent.mkdir(parents=True)
            binary.write_text("binary", encoding="utf-8")
            with (
                mock.patch.object(helper, "ATTEMPT_BASE", attempts),
                mock.patch.object(helper, "PROTECTED_BASE", base / "protected"),
                mock.patch.object(helper, "STATE_BASE", base / "state"),
                mock.patch.object(helper, "ELECTRON_APP", electron),
                mock.patch.object(helper, "run") as run,
                self.assertRaisesRegex(helper.HelperError, "protected task store"),
            ):
                helper.launch_task_app(
                    "trial-one",
                    "example-desk",
                    "task-store",
                    '["--record=ITEM-1042"]',
                )
            run.assert_not_called()

    def test_task_app_launch_uses_shared_store_only_when_explicit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            attempts = base / "attempts"
            root = attempts / "trial-one"
            (root / "task/apps/example-desk").mkdir(parents=True)
            store = root / "workspace/task-store"
            store.mkdir(parents=True)
            electron = base / "Electron.app"
            binary = electron / "Contents/MacOS/Electron"
            binary.parent.mkdir(parents=True)
            binary.write_text("binary", encoding="utf-8")
            account = types.SimpleNamespace(pw_name="lume")
            calls: list[list[str]] = []

            def fixed_run(argv, **_kwargs):
                calls.append(argv)
                return types.SimpleNamespace(stdout=f"{os.getuid()}\n", stderr="")

            with (
                mock.patch.object(helper, "ATTEMPT_BASE", attempts),
                mock.patch.object(helper, "PROTECTED_BASE", base / "protected"),
                mock.patch.object(helper, "STATE_BASE", base / "state"),
                mock.patch.object(helper, "ELECTRON_APP", electron),
                mock.patch.object(helper, "run", side_effect=fixed_run),
                mock.patch.object(helper.pwd, "getpwuid", return_value=account),
                mock.patch.object(helper, "_task_app_processes", return_value=[4321]),
                mock.patch.object(helper, "_process_identity", return_value="a" * 64),
                mock.patch.object(helper.time, "sleep"),
            ):
                evidence = helper.launch_task_app(
                    "trial-one",
                    "example-desk",
                    "task-store",
                    '["--record=ITEM-1042"]',
                    "shared-agent-console",
                )
            self.assertEqual(evidence["store_mode"], "shared-agent-console")
            self.assertIn(f"--store={store}", calls[-1])

    def test_task_app_process_discovery_requires_exact_store_and_main_child(self) -> None:
        electron = Path("/opt/Electron.app/Contents/MacOS/Electron")
        application = Path("/attempt/task/apps/example-desk")
        store = "--store=/protected/trial-one/task-store"
        listing = "\n".join(
            (
                f"101 501 {electron} {application} {store} --record=ITEM-1042",
                f"102 501 {electron} /other/app {store} --record=ITEM-1042",
                f"103 501 {electron} {application} --store=/workspace/task-store",
                f"104 501 {electron} {application} {store} --type=renderer",
                f"105 502 {electron} {application} {store}",
            )
        )
        with mock.patch.object(
            helper.subprocess,
            "run",
            return_value=types.SimpleNamespace(returncode=0, stdout=listing),
        ):
            self.assertEqual(helper._task_app_processes(501, electron, application, store), [101])

    def test_protected_task_store_is_a_console_only_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            attempts = base / "attempts"
            protected = base / "protected"
            source = attempts / "trial-one/workspace/task-store"
            source.mkdir(parents=True)
            record = source / "records.json"
            record.write_text('{"record":"ITEM-1042"}\n', encoding="utf-8")
            account = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
            with (
                mock.patch.object(helper, "ATTEMPT_BASE", attempts),
                mock.patch.object(helper, "PROTECTED_BASE", protected),
                mock.patch.object(helper.pwd, "getpwnam", return_value=account),
                mock.patch.object(helper.os, "chown"),
            ):
                evidence = helper.prepare_task_store("trial-one", "task-store")
            snapshot = protected / "trial-one/task-store/records.json"
            self.assertEqual(evidence["mode"], "protected-console-only")
            self.assertEqual(snapshot.read_text(encoding="utf-8"), record.read_text())
            record.write_text("agent mutation\n", encoding="utf-8")
            self.assertNotEqual(snapshot.read_text(encoding="utf-8"), record.read_text())
            self.assertEqual(stat.S_IMODE(snapshot.stat().st_mode), 0o600)

    def test_mediator_start_readiness_failure_reaps_child_and_cleans_runtime(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            state_base = base / "state"
            protected_base = base / "protected"
            mediator_base = base / "mediator"
            backend_parent = base / "backend"
            backend_parent.mkdir()
            backend_path = backend_parent / "driver.sock"
            backend = __import__("socket").socket(
                __import__("socket").AF_UNIX, __import__("socket").SOCK_STREAM
            )
            backend.bind(str(backend_path))
            protected = protected_base / "trial-one"
            protected.mkdir(parents=True)
            store = protected / "task-store"
            store.mkdir()
            store_status = store.stat()
            state_base.mkdir()
            (state_base / "trial-one.json").write_text(
                json.dumps(
                    {
                        "attempt": "trial-one",
                        "task_app": {
                            "store_mode": "protected-console-only",
                            "store_relative": "task-store",
                            "target_pid": 300,
                            "target_process_identity": "a" * 64,
                            "console_uid": os.getuid(),
                            "store_device": store_status.st_dev,
                            "store_inode": store_status.st_ino,
                            "store_path_sha256": hashlib.sha256(
                                str(store.resolve()).encode("utf-8")
                            ).hexdigest(),
                        },
                    }
                ),
                encoding="utf-8",
            )
            account = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
            mediator_path = mock.MagicMock()
            mediator_path.stat.return_value = types.SimpleNamespace(
                st_mode=stat.S_IFREG | 0o555, st_uid=0, st_gid=0
            )
            mediator_path.__str__.return_value = "/fake/cdb-driver-mediator"
            try:
                with (
                    mock.patch.object(helper, "STATE_BASE", state_base),
                    mock.patch.object(helper, "PROTECTED_BASE", protected_base),
                    mock.patch.object(helper, "MEDIATOR_BASE", mediator_base),
                    mock.patch.object(helper, "DRIVER_SOCKET", backend_path),
                    mock.patch.object(helper, "MEDIATOR_PATH", mediator_path),
                    mock.patch.object(helper.pwd, "getpwnam", return_value=account),
                    mock.patch.object(helper.os, "chown"),
                    mock.patch.object(helper.os, "fork", return_value=123),
                    mock.patch.object(helper.os, "waitpid", return_value=(0, 0)),
                    mock.patch.object(helper, "_process_uid", side_effect=lambda pid: os.getuid()),
                    mock.patch.object(
                        helper,
                        "_process_identity",
                        side_effect=lambda pid: "a" * 64 if pid == 300 else None,
                    ),
                    mock.patch.object(helper.time, "monotonic", side_effect=[0.0, 6.0]),
                    mock.patch.object(helper, "_terminate_and_reap") as terminate,
                    mock.patch.object(helper, "_remove_mediator_runtime") as cleanup,
                    self.assertRaisesRegex(helper.HelperError, "did not become ready"),
                ):
                    helper.start_mediator("trial-one", "synthetic-task.v1", "d" * 64)
                terminate.assert_called_once_with(123)
                cleanup.assert_called_once_with(mediator_base / "trial-one")
            finally:
                backend.close()

    def test_mediator_stop_rejects_backend_socket_swap(self) -> None:
        self._assert_mediator_stop_identity_failure("backend", "backend socket")

    def test_mediator_stop_rejects_target_process_drift(self) -> None:
        self._assert_mediator_stop_identity_failure("target", "target identity")

    def test_non_child_mediator_exit_waits_for_exact_process_identity(self) -> None:
        identities = iter(["a" * 64, "a" * 64, None])
        with (
            mock.patch.object(
                helper, "_process_identity", side_effect=lambda _pid: next(identities)
            ) as inspect,
            mock.patch.object(helper.time, "sleep"),
        ):
            self.assertTrue(helper._wait_for_process_exit(200, "a" * 64, 1.0))
        self.assertEqual(inspect.call_count, 3)

    def test_non_child_mediator_pid_reuse_counts_as_original_exit(self) -> None:
        with mock.patch.object(helper, "_process_identity", return_value="b" * 64):
            self.assertTrue(helper._wait_for_process_exit(200, "a" * 64, 1.0))

    def _assert_mediator_stop_identity_failure(self, failure: str, expected_message: str) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            state_base = base / "state"
            mediator_base = base / "mediator"
            protected_base = base / "protected"
            backend_parent = base / "backend"
            state_base.mkdir()
            (mediator_base / "trial-one").mkdir(parents=True)
            backend_parent.mkdir()
            backend_path = backend_parent / "driver.sock"
            native_socket = __import__("socket")
            backend = native_socket.socket(native_socket.AF_UNIX, native_socket.SOCK_STREAM)
            frontend = native_socket.socket(native_socket.AF_UNIX, native_socket.SOCK_STREAM)
            backend.bind(str(backend_path))
            frontend_path = mediator_base / "trial-one/driver.sock"
            frontend.bind(str(frontend_path))
            store = protected_base / "trial-one/task-store"
            store.mkdir(parents=True)
            store_status = store.stat()
            backend_status = backend_path.lstat()
            parent_status = backend_parent.lstat()
            frontend_status = frontend_path.lstat()
            mediator = {
                "pid": 200,
                "worker_uid": 501,
                "process_identity": "a" * 64,
                "frontend_device": frontend_status.st_dev,
                "frontend_inode": frontend_status.st_ino,
                "backend_parent_device": parent_status.st_dev,
                "backend_parent_inode": parent_status.st_ino,
                "backend_device": backend_status.st_dev,
                "backend_inode": backend_status.st_ino + (1 if failure == "backend" else 0),
                "backend_parent_original_mode": 0o700,
                "target_pid": 300,
                "target_process_identity": "b" * 64,
                "sealed": False,
            }
            state = {
                "attempt": "trial-one",
                "task_app": {
                    "target_pid": 300,
                    "console_uid": 501,
                    "store_relative": "task-store",
                    "store_device": store_status.st_dev,
                    "store_inode": store_status.st_ino,
                    "store_path_sha256": hashlib.sha256(
                        str(store.resolve()).encode("utf-8")
                    ).hexdigest(),
                },
                "mediator": mediator,
            }
            (state_base / "trial-one.json").write_text(json.dumps(state), encoding="utf-8")
            identities = {200: "a" * 64, 300: "c" * 64 if failure == "target" else "b" * 64}
            try:
                with (
                    mock.patch.object(helper, "STATE_BASE", state_base),
                    mock.patch.object(helper, "MEDIATOR_BASE", mediator_base),
                    mock.patch.object(helper, "PROTECTED_BASE", protected_base),
                    mock.patch.object(helper, "DRIVER_SOCKET", backend_path),
                    mock.patch.object(helper, "_process_uid", return_value=501),
                    mock.patch.object(
                        helper, "_process_identity", side_effect=lambda pid: identities[pid]
                    ),
                    mock.patch.object(helper, "_terminate_and_reap") as terminate,
                    mock.patch.object(helper, "_remove_mediator_runtime"),
                    mock.patch.object(helper.os, "chmod"),
                    self.assertRaisesRegex(helper.HelperError, expected_message),
                ):
                    helper.stop_and_seal_mediator("trial-one")
                terminate.assert_called_once_with(200)
            finally:
                backend.close()
                frontend.close()

    def test_sealed_mediator_log_requires_an_exact_hash_chain(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mediator.ndjson"
            previous = "0" * 64
            records = []
            for sequence, fields in enumerate(
                (
                    {
                        "kind": "exchange",
                        "request_sha256": "a" * 64,
                        "event": "observación café",
                    },
                    {
                        "kind": "seal",
                        "fatal": False,
                        "evidence_complete": False,
                        "certifying": False,
                        "tool_contract_validated": False,
                        "tool_contract_required": False,
                    },
                ),
                1,
            ):
                body = {
                    "seq": sequence,
                    "prev": previous,
                    "attempt": "trial-one",
                    "task": "synthetic-task.v1",
                    **fields,
                }
                record_hash = hashlib.sha256(
                    json.dumps(
                        body,
                        sort_keys=True,
                        separators=(",", ":"),
                        ensure_ascii=False,
                    ).encode("utf-8")
                ).hexdigest()
                record = {**body, "hash": record_hash}
                records.append(record)
                previous = record_hash
            path.write_text(
                "".join(
                    json.dumps(
                        row,
                        sort_keys=True,
                        separators=(",", ":"),
                        ensure_ascii=False,
                    )
                    + "\n"
                    for row in records
                ),
                encoding="utf-8",
            )
            parsed, digest = helper._validate_chained_log(
                path, "trial-one", expected_owner_uid=os.getuid()
            )
            self.assertEqual(parsed, records)
            self.assertIn("café".encode("utf-8"), path.read_bytes())
            self.assertEqual(digest, hashlib.sha256(path.read_bytes()).hexdigest())
            records[0]["request_sha256"] = "b" * 64
            path.write_text(
                "".join(
                    json.dumps(
                        row,
                        sort_keys=True,
                        separators=(",", ":"),
                        ensure_ascii=False,
                    )
                    + "\n"
                    for row in records
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(helper.HelperError, "hash mismatch"):
                helper._validate_chained_log(path, "trial-one", expected_owner_uid=os.getuid())

    def test_off_target_reason_counts_are_bounded_and_content_free(self) -> None:
        records = [
            {"kind": "exchange"},
            {
                "kind": "anomaly",
                "anomaly": "off_target_gui_activity",
                "reason": "target_input_unbound",
            },
            {
                "kind": "anomaly",
                "anomaly": "off_target_gui_activity",
                "reason": "tool_unapproved",
            },
            {
                "kind": "anomaly",
                "anomaly": "off_target_gui_activity",
                "reason": "target_input_unbound",
            },
        ]
        self.assertEqual(
            helper._off_target_reason_counts(records),
            {"target_input_unbound": 2, "tool_unapproved": 1},
        )

    def test_chained_log_rejects_unbounded_anomaly_reason(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mediator.ndjson"
            previous = "0" * 64
            records = []
            for sequence, fields in enumerate(
                (
                    {
                        "kind": "anomaly",
                        "anomaly": "off_target_gui_activity",
                        "reason": "raw request: secret",
                    },
                    {
                        "kind": "seal",
                        "fatal": False,
                        "evidence_complete": False,
                        "certifying": False,
                        "tool_contract_validated": False,
                        "tool_contract_required": False,
                    },
                ),
                1,
            ):
                body = {
                    "seq": sequence,
                    "prev": previous,
                    "attempt": "trial-one",
                    "task": "synthetic-task.v1",
                    **fields,
                }
                claimed = hashlib.sha256(
                    json.dumps(
                        body,
                        sort_keys=True,
                        separators=(",", ":"),
                    ).encode()
                ).hexdigest()
                records.append({**body, "hash": claimed})
                previous = claimed
            path.write_text(
                "".join(
                    json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n"
                    for record in records
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(helper.HelperError, "anomaly evidence is invalid"):
                helper._validate_chained_log(path, "trial-one", expected_owner_uid=os.getuid())

    def test_durable_json_fsyncs_file_and_parent_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "participation.sealed.json"
            real_fsync = os.fsync
            synced_modes: list[int] = []

            def record_fsync(descriptor: int) -> None:
                synced_modes.append(os.fstat(descriptor).st_mode)
                real_fsync(descriptor)

            with mock.patch.object(helper.os, "fsync", side_effect=record_fsync):
                helper._write_durable_json_new(path, {"certifying": True})

            self.assertEqual(
                json.loads(path.read_text(encoding="utf-8")),
                {"certifying": True},
            )
            self.assertEqual(len(synced_modes), 2)
            self.assertTrue(stat.S_ISREG(synced_modes[0]))
            self.assertTrue(stat.S_ISDIR(synced_modes[1]))
            with self.assertRaises(FileExistsError):
                helper._write_durable_json_new(path, {"certifying": False})


if __name__ == "__main__":
    unittest.main()
