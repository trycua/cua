# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The docs arm (pi-raw-macos-docs; Zane, 13 Nov 2026 21:58 SGT): the generic notes name no bench app, task or trial;
the lookup ranks and renders deterministically; the server lists exactly run_script and lookup; and a lookup query is
not scanned as code by the GUI-only guard. The generator itself needs the macOS SDK; its determinism is checked in
the bench guest by running it twice and diffing."""

from __future__ import annotations

import importlib
import json
import os
import re
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tools" / "raw_macos"))
import docs_lookup  # noqa: E402

TASKPACK = HERE.parents[1] / "taskpack"
NOTES = HERE / "tools" / "raw_macos" / "docs" / "notes.md"
# Products the task pack's `apps` fields describe in words, and the harness's own apps (desktop_runtime, claude_arms)
PRODUCTS = (
    "LibreOffice",
    "Calc",
    "GnuCash",
    "Electron",
    "Chrome",
    "Chromium",
    "BenchLab",
    "Bench Sentinel",
    "BenchSentinel",
    "Sentinel",
)


def banned_words() -> set[str]:
    """Every bench app, task id and run name the notes must not contain, read from the task pack and probes."""
    words = set(PRODUCTS)
    taskset = json.loads((TASKPACK / "v2-taskset.json").read_text("utf-8"))
    for task in taskset["tasks"]:
        words.add(task["id"])
        if task.get("pack_task"):
            words.add(task["pack_task"].rsplit("/", 1)[-1])
    for app_dir in TASKPACK.glob("tasks/*/*/apps/*"):
        words.add(app_dir.name)
        words.update(p for p in app_dir.name.split("-") if len(p) > 4)  # lighthouse, rillwatch, ...
    for meta in (HERE / "probes").glob("*/task.json"):
        data = json.loads(meta.read_text("utf-8"))
        words.add(data["id"])
        args = data.get("app_args") or []
        words.update(a for a in args if not a.startswith("-"))  # BenchLab modes
    return words


def lint_notes(text: str) -> list[str]:
    hits = []
    for word in sorted(banned_words()):
        pattern = r"(?<![A-Za-z0-9])" + re.escape(word) + r"(?![A-Za-z0-9])"
        if re.search(pattern, text, re.IGNORECASE):
            hits.append(word)
    if re.search(r"\b(MB|IR|CDB|WEB)-[A-Z]?\d{2}\b", text):
        hits.append("task id pattern")
    if re.search(r"\b(trial|bench|run index)\b", text, re.IGNORECASE):
        hits.append("trial/bench wording")
    return hits


class NotesLintTest(unittest.TestCase):
    def test_notes_name_no_bench_app_task_or_trial(self):
        self.assertEqual(lint_notes(NOTES.read_text("utf-8")), [])

    def test_lint_catches_each_kind(self):
        for text in (
            "Open it in GnuCash.",
            "as in MB-09",
            "the lighthouse-desk window",
            "listdrag mode",
            "LibreOffice drops keys",
            "in a trial",
            "cdb-s04",
        ):
            with self.subTest(text=text):
                self.assertNotEqual(lint_notes(text), [])

    def test_banned_list_comes_from_the_pack(self):
        words = banned_words()
        for word in ("CDB-G02", "MB-11", "IR-04", "rillwatch-cases", "tooltip"):
            self.assertIn(word, words)

    def test_notes_are_small_and_split_into_entries(self):
        notes = docs_lookup.load_notes(NOTES)
        self.assertGreaterEqual(len(notes), 8)
        self.assertLess(len(NOTES.read_text("utf-8")), 16_000)
        self.assertTrue(all(n["doc"] for n in notes))


ENTRIES = [
    {
        "framework": "ImageIO",
        "kind": "func",
        "name": "CGImageDestinationCreateWithURL",
        "decl": "extern CGImageDestinationRef CGImageDestinationCreateWithURL(CFURLRef url, CFStringRef type, size_t count, "
        "CFDictionaryRef options);",
        "doc": "Create an image destination writing to `url'.",
    },
    {
        "framework": "ImageIO",
        "kind": "func",
        "name": "CGImageDestinationFinalize",
        "decl": "extern bool CGImageDestinationFinalize(CGImageDestinationRef idst);",
        "doc": "Write everything.",
    },
    {
        "framework": "AppKit",
        "kind": "method",
        "name": "NSWorkspace.openURLs:withApplicationAtURL:configuration:completionHandler:",
        "pyobjc": "openURLs_withApplicationAtURL_configuration_completionHandler_",
        "decl": "- (void) openURLs:...",
        "doc": "",
    },
    {
        "framework": "SkyLight (private)",
        "kind": "export",
        "name": "SLEventPostToPid",
        "decl": "",
        "doc": "Exported by the framework binary; no public header declares it, so the signature is not published.",
    },
    {
        "framework": "CoreGraphics (Quartz)",
        "kind": "func",
        "name": "CGEventPostToPid",
        "decl": "extern void CGEventPostToPid(pid_t pid, CGEventRef event);",
        "doc": "x " * 800,
    },
]


class LookupTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        (root / "TEST1").mkdir()
        (root / "TEST1" / "index.jsonl").write_text(
            "\n".join(json.dumps(e) for e in ENTRIES) + "\n", "utf-8"
        )
        self.index = docs_lookup.Index.load(root, "TEST1")

    def tearDown(self):
        self.tmp.cleanup()

    def names(self, query: str, limit: int = 8) -> list[str]:
        return [e["name"] for e in self.index.search(query, limit)]

    def test_exact_name_ranks_first(self):
        self.assertEqual(self.names("SLEventPostToPid")[0], "SLEventPostToPid")
        self.assertEqual(self.names("_SLEventPostToPid")[0], "SLEventPostToPid")
        self.assertEqual(
            self.names("openURLs_withApplicationAtURL_configuration_completionHandler_")[0],
            ENTRIES[2]["name"],
        )

    def test_words_find_notes_and_camel_case_parts(self):
        found = self.names("save image to disk png")
        self.assertEqual(found[0], "Save an image to disk")
        self.assertIn("CGImageDestinationCreateWithURL", self.names("image destination"))

    def test_same_query_same_answer_and_limit(self):
        self.assertEqual(self.names("post event pid"), self.names("post event pid"))
        self.assertEqual(len(self.names("event", 2)), 2)
        self.assertLessEqual(len(self.names("a", 999)), docs_lookup.MAX_LIMIT)
        self.assertEqual(self.index.search("zzzzqqq"), [])

    def test_render_cuts_long_api_docs_not_notes(self):
        text = docs_lookup.render(self.index.search("CGEventPostToPid", 1))
        self.assertIn("extern void CGEventPostToPid", text)
        self.assertLess(len(text), docs_lookup.DOC_CHARS + 300)
        note = docs_lookup.render(self.index.search("Drag in a background window", 1))
        self.assertTrue(note.startswith("[note] Drag in a background window"))
        self.assertIn("gesture id", note)
        self.assertEqual(docs_lookup.render([]), "No matching entries.")

    def test_missing_build_is_an_error(self):
        with self.assertRaises(FileNotFoundError):
            docs_lookup.Index.load(Path(self.tmp.name), "NOPE")


class DocsServerTest(unittest.TestCase):
    def server(self, docs: str | None):
        os.environ["RAW_BARE"] = "1"
        if docs:
            os.environ["RAW_DOCS_DIR"] = docs
        try:
            import raw_mcp_mac

            return importlib.reload(raw_mcp_mac)
        finally:
            os.environ.pop("RAW_BARE", None)
            os.environ.pop("RAW_DOCS_DIR", None)

    def tearDown(self):
        import raw_mcp_mac

        importlib.reload(raw_mcp_mac)

    def test_lookup_returns_entries_and_logs_the_query(self):
        with tempfile.TemporaryDirectory() as tmp:
            build = (
                docs_lookup.os_build()
                if Path("/System/Library/CoreServices/SystemVersion.plist").is_file()
                else None
            )
            if build is None:
                self.skipTest("needs macOS's SystemVersion.plist")
            (Path(tmp) / build).mkdir()
            (Path(tmp) / build / "index.jsonl").write_text(json.dumps(ENTRIES[3]) + "\n", "utf-8")
            server = self.server(tmp)
            content, failed, entry = server.lookup("SLEventPostToPid", None)
        self.assertFalse(failed)
        self.assertIn("SLEventPostToPid", content[0]["text"])
        self.assertEqual(entry["tool"], "lookup")
        self.assertEqual(entry["hits"][0], "SLEventPostToPid")

    def test_bare_server_has_no_lookup_and_same_description(self):
        bare = self.server(None)
        bare_dir, bare_text = bare.DOCS_DIR, bare.description({})
        docs = self.server("/nonexistent")
        self.assertIsNone(bare_dir)
        self.assertEqual(docs.DOCS_DIR, Path("/nonexistent"))
        self.assertEqual(bare_text, docs.description({}))
        self.assertLess(len(docs.LOOKUP_DESCRIPTION), 300)


class HarnessTest(unittest.TestCase):
    def test_lookup_query_is_not_scanned_as_code(self):
        import bench_v2

        hard, _ = bench_v2.scan_tool_use(
            "mcp__cua__lookup", {"query": "subprocess NSTask osascript"}
        )
        self.assertEqual(hard, [])
        hard, _ = bench_v2.scan_tool_use("mcp__cua__run_script", {"script": "import subprocess"})
        self.assertNotEqual(hard, [])

    def test_prompt_adds_one_line_only(self):
        import claude_arms as ca

        bare = ca.system_prompt_for(ca.RAW_MACOS_BARE_ARM)
        docs = ca.system_prompt_for(ca.RAW_MACOS_DOCS_ARM)
        self.assertTrue(docs.startswith(bare))
        extra = docs[len(bare) :].strip()
        self.assertEqual(extra.count("\n"), 0)
        self.assertIn("lookup", extra)
        self.assertEqual(lint_notes(extra), [])

    def test_adapter_env(self):
        import claude_arms as ca

        env = ca.adapter_entry(ca.RAW_MACOS_DOCS_ARM)["env"]
        self.assertEqual(env["RAW_BARE"], "1")
        self.assertEqual(env["RAW_NATIVE"], "1")
        self.assertTrue(env["RAW_DOCS_DIR"])
        self.assertEqual(ca.adapter_entry(ca.RAW_MACOS_BARE_ARM)["env"]["RAW_DOCS_DIR"], "")


if __name__ == "__main__":
    unittest.main()
