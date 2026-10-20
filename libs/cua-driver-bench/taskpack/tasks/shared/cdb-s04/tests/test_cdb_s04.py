"""Adversarial tests for the bounded, final-state CDB-S04 evaluator."""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
FIXTURE = BUNDLE / "fixture"
EVALUATE = BUNDLE / "evaluator" / "evaluate.py"
SETUP = BUNDLE / "reset" / "setup.py"
VERIFY = BUNDLE / "reset" / "verify.py"
ROOT = "RWC-184"
EXPECTED = {
    "RWC-193": {
        "wiki_ref": "MLM-27#legacy-empty-layers",
        "repro_url": "http://127.0.0.1:4184/inspect?record=vesper-17&view=overlay",
        "log_ref": "arc-7f31:overlay_route:legacy-empty-layers",
    },
    "RWC-201": {
        "wiki_ref": "MLM-27#legacy-empty-layers",
        "repro_url": "http://127.0.0.1:4184/inspect?record=vesper-21&view=overlay",
        "log_ref": "arc-9b04:overlay_route:legacy-empty-layers",
    },
}

EVALUATOR_SPEC = importlib.util.spec_from_file_location("cdb_s04_evaluate", EVALUATE)
assert EVALUATOR_SPEC is not None and EVALUATOR_SPEC.loader is not None
EVALUATOR_MODULE = importlib.util.module_from_spec(EVALUATOR_SPEC)
EVALUATOR_SPEC.loader.exec_module(EVALUATOR_MODULE)


def run(command: list[str], *, check: bool = True) -> subprocess.CompletedProcess:
    completed = subprocess.run(command, capture_output=True, text=True, timeout=45)
    if check and completed.returncode != 0:
        raise AssertionError(f"command failed: {command}\n{completed.stdout}\n{completed.stderr}")
    return completed


class Workspace:
    """Fresh fixture copy, deliberately independent of reset/setup.py."""

    def __init__(self, temporary: Path):
        self.root = temporary / "workspace"
        self.artifacts = temporary / "artifacts"
        shutil.copytree(FIXTURE, self.root)
        self.artifacts.mkdir()
        for name in ("browser", "cases", "wiki"):
            (self.root / ".runtime" / name).mkdir(parents=True, exist_ok=True)

    @property
    def cases_path(self) -> Path:
        return self.root / "cases" / "cases.json"

    def read_cases(self) -> dict:
        return json.loads(self.cases_path.read_text(encoding="utf-8"))

    def write_cases(self, document: dict) -> None:
        self.cases_path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")

    def case(self, document: dict, case_id: str) -> dict:
        return next(case for case in document["cases"] if case["id"] == case_id)

    def link(self, case_id: str, *, root: str = ROOT, evidence: dict | None = None) -> None:
        document = self.read_cases()
        case = self.case(document, case_id)
        case["status"] = "duplicate"
        case["duplicate_of"] = root
        case["evidence"] = dict(EXPECTED[case_id] if evidence is None else evidence)
        self.write_cases(document)

    def complete(self) -> None:
        self.link("RWC-193")
        self.link("RWC-201")

    def evaluate(self) -> dict:
        result = self.root.parent / "result.json"
        completed = run([
            sys.executable, str(EVALUATE), "--workspace", str(self.root),
            "--artifacts", str(self.artifacts), "--result", str(result),
            "--agent-exit-code", "0",
        ])
        self.stderr = completed.stderr
        return json.loads(result.read_text(encoding="utf-8"))


class EvaluatorCase(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="cdb-s04-test-")
        self.ws = Workspace(Path(self.temporary.name))

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def assert_check(self, result: dict, name: str, passed: bool) -> None:
        check = result["detail"]["checks"][name]
        self.assertEqual(check["passed"], passed, json.dumps(check, indent=2)[:2000])

    def test_canonical_success(self) -> None:
        self.ws.complete()
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:6000])
        self.assertEqual(result["score"], 1.0)
        self.assertFalse(result["detail"]["artifacts_consulted"])

    def test_no_op_fails_without_candidate_correlations(self) -> None:
        result = self.ws.evaluate()
        self.assertFalse(result["passed"])
        self.assert_check(result, "duplicate_links_correct", False)
        self.assert_check(result, "browser_reproduction_semantics", False)
        self.assert_check(result, "terminal_log_correlation", False)
        self.assert_check(result, "preservation", True)

    def test_wrong_root_fails(self) -> None:
        self.ws.complete()
        document = self.ws.read_cases()
        self.ws.case(document, "RWC-193")["duplicate_of"] = "RWC-177"
        self.ws.write_cases(document)
        result = self.ws.evaluate()
        self.assert_check(result, "duplicate_links_correct", False)

    def test_only_one_duplicate_fails(self) -> None:
        self.ws.link("RWC-193")
        result = self.ws.evaluate()
        self.assert_check(result, "duplicate_links_correct", False)
        self.assert_check(result, "evidence_complete_and_correlated", False)

    def test_swapped_correlations_fail(self) -> None:
        self.ws.link("RWC-193", evidence=EXPECTED["RWC-201"])
        self.ws.link("RWC-201", evidence=EXPECTED["RWC-193"])
        result = self.ws.evaluate()
        self.assert_check(result, "evidence_complete_and_correlated", False)
        self.assert_check(result, "terminal_log_correlation", False)

    def test_copied_correlation_fails(self) -> None:
        self.ws.link("RWC-193")
        self.ws.link("RWC-201", evidence=EXPECTED["RWC-193"])
        result = self.ws.evaluate()
        self.assert_check(result, "evidence_complete_and_correlated", False)
        self.assert_check(result, "terminal_log_correlation", False)

    def test_missing_one_evidence_field_fails(self) -> None:
        self.ws.complete()
        document = self.ws.read_cases()
        del self.ws.case(document, "RWC-193")["evidence"]["log_ref"]
        self.ws.write_cases(document)
        self.assert_check(self.ws.evaluate(), "evidence_complete_and_correlated", False)

    def test_plausible_near_miss_linked_and_closed_fails(self) -> None:
        self.ws.complete()
        document = self.ws.read_cases()
        near_miss = self.ws.case(document, "RWC-206")
        near_miss.update({
            "status": "duplicate", "duplicate_of": ROOT,
            "evidence": dict(EXPECTED["RWC-193"]),
        })
        self.ws.write_cases(document)
        result = self.ws.evaluate()
        self.assert_check(result, "unsupported_cases_open", False)
        self.assert_check(result, "preservation", False)

    def test_reversed_or_chain_link_fails(self) -> None:
        self.ws.complete()
        document = self.ws.read_cases()
        self.ws.case(document, "RWC-193")["duplicate_of"] = "RWC-201"
        self.ws.write_cases(document)
        self.assert_check(self.ws.evaluate(), "duplicate_links_correct", False)

    def test_root_decoy_and_immutable_mutations_fail_preservation(self) -> None:
        for case_id, field in (("RWC-184", "title"), ("RWC-177", "body"), ("RWC-193", "body")):
            with self.subTest(case_id=case_id):
                with tempfile.TemporaryDirectory(prefix="cdb-s04-mutation-") as temporary:
                    ws = Workspace(Path(temporary))
                    ws.complete()
                    document = ws.read_cases()
                    ws.case(document, case_id)[field] += " altered"
                    ws.write_cases(document)
                    self.assert_check(ws.evaluate(), "preservation", False)

    def test_wiki_log_and_site_mutations_fail_preservation(self) -> None:
        targets = ["wiki/articles.json", "logs/archive-access.log", "archive-site/server.js"]
        for relative in targets:
            with self.subTest(relative=relative):
                with tempfile.TemporaryDirectory(prefix="cdb-s04-source-") as temporary:
                    ws = Workspace(Path(temporary))
                    ws.complete()
                    path = ws.root / relative
                    path.write_bytes(path.read_bytes() + b"\n")
                    self.assert_check(ws.evaluate(), "preservation", False)

    def test_extra_file_fails_preservation(self) -> None:
        self.ws.complete()
        (self.ws.root / "claim.txt").write_text("all cases verified\n", encoding="utf-8")
        self.assert_check(self.ws.evaluate(), "preservation", False)

    def test_symlink_in_ignored_directory_is_not_followed_or_graded(self) -> None:
        self.ws.complete()
        target = self.ws.root / ".runtime" / "linked"
        try:
            target.symlink_to(self.ws.root / "cases")
        except (OSError, NotImplementedError):
            self.skipTest("symlinks unavailable")
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:4000])

    def test_many_ignored_runtime_files_do_not_fail_preservation(self) -> None:
        self.ws.complete()
        browser_runtime = self.ws.root / ".runtime" / "browser"
        for index in range(1100):
            (browser_runtime / f"cache-{index:04d}").write_bytes(b"x")
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:4000])

    def test_benign_os_metadata_is_ignored(self) -> None:
        self.ws.complete()
        (self.ws.root / ".DS_Store").write_bytes(b"metadata")
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:4000])

    def test_malformed_and_oversized_case_json_are_contained(self) -> None:
        for payload in (b"{broken", b" " * (1024 * 1024 + 1)):
            with self.subTest(size=len(payload)):
                with tempfile.TemporaryDirectory(prefix="cdb-s04-json-") as temporary:
                    ws = Workspace(Path(temporary))
                    ws.cases_path.write_bytes(payload)
                    result = ws.evaluate()
                    self.assertFalse(result["passed"])
                    self.assert_check(result, "duplicate_links_correct", False)
                    self.assertEqual(result["score"], 0.0 if len(payload) > 4 * 1024 * 1024 else result["score"])

    def test_browser_and_log_only_claims_do_not_help(self) -> None:
        (self.ws.artifacts / "browser.png").write_bytes(b"fake screenshot")
        (self.ws.artifacts / "terminal.log").write_text(
            "arc-7f31:overlay_route:legacy-empty-layers\n", encoding="utf-8"
        )
        result = self.ws.evaluate()
        self.assertFalse(result["passed"])
        self.assertFalse(result["detail"]["artifacts_consulted"])
        self.assert_check(result, "duplicate_links_correct", False)

    def test_ticket_only_claim_fails_without_browser_and_log_sources(self) -> None:
        self.ws.complete()
        (self.ws.root / "archive-site" / "server.js").write_text(
            'process.exit(2);\n', encoding="utf-8"
        )
        (self.ws.root / "logs" / "archive-access.log").write_text("claim only\n", encoding="utf-8")
        result = self.ws.evaluate()
        self.assert_check(result, "duplicate_links_correct", True)
        self.assert_check(result, "browser_reproduction_semantics", False)
        self.assert_check(result, "terminal_log_correlation", False)

    def test_server_failure_is_bounded_and_fails(self) -> None:
        self.ws.complete()
        marker = self.ws.root.parent / "submission-server-ran"
        (self.ws.root / "archive-site" / "server.js").write_text(
            f'require("fs").writeFileSync({json.dumps(str(marker))}, "ran");\n', encoding="utf-8"
        )
        result = self.ws.evaluate()
        self.assert_check(result, "browser_reproduction_semantics", False)
        self.assertFalse(marker.exists(), "workspace-modifiable server code was executed")

    def test_evaluator_ignores_planted_results(self) -> None:
        planted = json.dumps({"passed": True, "score": 1.0})
        for path in (self.ws.artifacts / "result.json", self.ws.artifacts / "report.md"):
            path.write_text(planted, encoding="utf-8")
        result = self.ws.evaluate()
        self.assertFalse(result["passed"])
        self.assertFalse(result["detail"]["artifacts_consulted"])

    def test_noncanonical_repro_authority_fails(self) -> None:
        for replacement in ("127.0.0.1:59999", "localhost:4184"):
            with self.subTest(authority=replacement):
                with tempfile.TemporaryDirectory(prefix="cdb-s04-authority-") as temporary:
                    ws = Workspace(Path(temporary))
                    ws.complete()
                    document = ws.read_cases()
                    for case_id in EXPECTED:
                        evidence = ws.case(document, case_id)["evidence"]
                        evidence["repro_url"] = evidence["repro_url"].replace(
                            "127.0.0.1:4184", replacement
                        )
                    ws.write_cases(document)
                    result = ws.evaluate()
                    self.assert_check(result, "evidence_complete_and_correlated", False)
                    self.assert_check(result, "browser_reproduction_semantics", False)

    def test_safe_reference_whitespace_is_normalized(self) -> None:
        self.ws.complete()
        document = self.ws.read_cases()
        for case_id in EXPECTED:
            evidence = self.ws.case(document, case_id)["evidence"]
            evidence["wiki_ref"] = f"  {evidence['wiki_ref']}\t"
            evidence["log_ref"] = f"\n{evidence['log_ref']}  "
        self.ws.write_cases(document)
        self.assertTrue(self.ws.evaluate()["passed"])

    def test_process_lifecycle_helpers_cover_windows_and_posix(self) -> None:
        self.assertEqual(EVALUATOR_MODULE._process_group_options("posix"), {"start_new_session": True})
        self.assertNotEqual(
            EVALUATOR_MODULE._process_group_options("nt")["creationflags"] & 0x200, 0
        )

        class WindowsProcess:
            pid = 42

            def __init__(self) -> None:
                self.signals = []
                self.waits = []

            def poll(self):
                return None

            def send_signal(self, sent):
                self.signals.append(sent)

            def wait(self, timeout):
                self.waits.append(timeout)

            def terminate(self):
                raise AssertionError("Windows group signal unexpectedly failed")

        process = WindowsProcess()
        EVALUATOR_MODULE._terminate(process, "nt")
        self.assertEqual(len(process.signals), 1)
        self.assertEqual(process.waits, [3])


class ResetCase(unittest.TestCase):
    def test_setup_is_deterministic_and_verify_accepts_fresh_state(self) -> None:
        with tempfile.TemporaryDirectory(prefix="cdb-s04-reset-") as temporary:
            workspace = Path(temporary) / "workspace"
            first = run([sys.executable, str(SETUP), "--workspace", str(workspace)])
            first_tree = {
                path.relative_to(workspace).as_posix(): path.read_bytes()
                for path in workspace.rglob("*") if path.is_file()
            }
            second = run([sys.executable, str(SETUP), "--workspace", str(workspace)])
            second_tree = {
                path.relative_to(workspace).as_posix(): path.read_bytes()
                for path in workspace.rglob("*") if path.is_file()
            }
            verified = run([sys.executable, str(VERIFY), "--workspace", str(workspace)])
            self.assertEqual(first_tree, second_tree)
            self.assertTrue(json.loads(first.stdout)["ok"])
            self.assertTrue(json.loads(second.stdout)["ok"])
            self.assertTrue(json.loads(verified.stdout)["ok"])
            facts_bytes = (workspace / "cdb-s04.workspace.json").read_bytes()
            self.assertTrue(facts_bytes.endswith(b"\n"))
            self.assertNotIn(b"\r\n", facts_bytes)

    def test_setup_removes_stray_root_entries_and_restores_fixture(self) -> None:
        with tempfile.TemporaryDirectory(prefix="cdb-s04-reset-strays-") as temporary:
            workspace = Path(temporary) / "workspace"
            run([sys.executable, str(SETUP), "--workspace", str(workspace)])
            (workspace / "stray.txt").write_text("stray\n", encoding="utf-8")
            (workspace / "stray-dir").mkdir()
            (workspace / "stray-dir" / "nested.txt").write_text("nested\n", encoding="utf-8")
            link = workspace / "stray-link"
            try:
                link.symlink_to(workspace / "cases", target_is_directory=True)
            except (OSError, NotImplementedError):
                link = None

            run([sys.executable, str(SETUP), "--workspace", str(workspace)])
            self.assertFalse((workspace / "stray.txt").exists())
            self.assertFalse((workspace / "stray-dir").exists())
            if link is not None:
                self.assertFalse(link.exists() or link.is_symlink())
            verified = run([sys.executable, str(VERIFY), "--workspace", str(workspace)])
            self.assertTrue(json.loads(verified.stdout)["ok"])

    def test_verify_rejects_stale_case_and_site_artifacts(self) -> None:
        targets = {
            "cases/cases.json": BUNDLE / "reset" / "failure-fixtures" / "stale-linked-case.json",
            "archive-site/stale.txt": BUNDLE / "reset" / "failure-fixtures" / "stale-site-file.txt",
        }
        for relative, source in targets.items():
            with self.subTest(relative=relative):
                with tempfile.TemporaryDirectory(prefix="cdb-s04-stale-") as temporary:
                    workspace = Path(temporary) / "workspace"
                    run([sys.executable, str(SETUP), "--workspace", str(workspace)])
                    target = workspace / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(source.read_bytes())
                    completed = run(
                        [sys.executable, str(VERIFY), "--workspace", str(workspace)],
                        check=False,
                    )
                    self.assertEqual(completed.returncode, 1)
                    self.assertFalse(json.loads(completed.stdout)["ok"])


if __name__ == "__main__":
    unittest.main()
