from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from cua_bench_runtime.collect import collect_tree
from cua_bench_runtime.errors import HarnessFailure
from cua_bench_runtime.guest_launch import CollectionBounds


class CollectionTests(unittest.TestCase):
    def test_copies_regular_files_and_skips_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            destination = root / "destination"
            (source / "nested").mkdir(parents=True)
            (source / "nested/data.txt").write_text("bounded", encoding="utf-8")
            (source / "escape").symlink_to("/etc/passwd")
            report = collect_tree(source, destination, CollectionBounds())
            self.assertEqual((destination / "nested/data.txt").read_text(), "bounded")
            self.assertFalse((destination / "escape").exists())
            self.assertEqual(report.skipped_symlinks, ("escape",))
            self.assertEqual(report.files[0].path, "nested/data.txt")
            self.assertEqual(report.total_bytes, 7)

    def test_limits_fail_closed_with_named_bound(self) -> None:
        cases = (
            (CollectionBounds(max_files=1), {"a": b"a", "b": b"b"}, "max_files"),
            (
                CollectionBounds(max_total_bytes=1, max_file_bytes=1),
                {"a": b"a", "b": b"b"},
                "max_total_bytes",
            ),
            (
                CollectionBounds(max_total_bytes=10, max_file_bytes=1),
                {"a": b"ab"},
                "max_file_bytes",
            ),
        )
        for index, (bounds, files, message) in enumerate(cases):
            with self.subTest(index=index), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "source"
                source.mkdir()
                for name, content in files.items():
                    (source / name).write_bytes(content)
                with self.assertRaisesRegex(HarnessFailure, message):
                    collect_tree(source, root / "destination", bounds)

    def test_depth_and_special_files_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            (source / "a/b").mkdir(parents=True)
            (source / "a/b/file").write_bytes(b"x")
            with self.assertRaisesRegex(HarnessFailure, "max_depth"):
                collect_tree(
                    source,
                    root / "destination",
                    CollectionBounds(max_depth=1),
                )

    def test_symlinked_source_or_ancestor_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trusted = root / "trusted"
            outside = root / "outside"
            trusted.mkdir()
            outside.mkdir()
            (outside / "secret").write_text("do not collect", encoding="utf-8")
            (trusted / "workspace").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(HarnessFailure, "symlink"):
                collect_tree(
                    trusted / "workspace",
                    root / "destination",
                    CollectionBounds(),
                    trusted_root=trusted,
                )

            (trusted / "workspace").unlink()
            (trusted / "attempt").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(HarnessFailure, "symlink"):
                collect_tree(
                    trusted / "attempt/workspace",
                    root / "destination-2",
                    CollectionBounds(),
                    trusted_root=trusted,
                )


if __name__ == "__main__":
    unittest.main()
