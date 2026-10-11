"""Adversarial, reset, and bundle tests for CDB-S02."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
REPO_ROOT = BUNDLE.parents[2]
SETUP = BUNDLE / "reset" / "setup.py"
VERIFY = BUNDLE / "reset" / "verify.py"
EVALUATE = BUNDLE / "evaluator" / "evaluate.py"
STORE_JS = BUNDLE / "apps" / "lighthouse-desk" / "store.js"
ORACLE = json.loads((BUNDLE / "evaluator" / "oracle.json").read_text())
FAILURES = BUNDLE / "reset" / "failure-fixtures"

NS = {
    "office": "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
    "table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
    "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
}
for prefix, uri in NS.items():
    ET.register_namespace(prefix, uri)
TABLE = "{" + NS["table"] + "}"
TEXT = "{" + NS["text"] + "}"
OFFICE = "{" + NS["office"] + "}"
GENESIS = "signal-quay-chat-genesis"


def run(command, check=True):
    completed = subprocess.run(command, capture_output=True, text=True)
    if check and completed.returncode != 0:
        raise AssertionError(f"command failed: {command}\n{completed.stdout}\n{completed.stderr}")
    return completed


def canonical(value) -> str:
    if isinstance(value, list):
        return "[" + ",".join(canonical(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{" + ",".join(json.dumps(key) + ":" + canonical(value[key]) for key in sorted(value)) + "}"
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return "null"
    if isinstance(value, (int, float)):
        return json.dumps(value)
    return json.dumps(value)


def write_ods_rows(path: Path, updates: dict[str, list[str]]) -> None:
    with zipfile.ZipFile(path) as source:
        members = [(info, source.read(info.filename)) for info in source.infolist()]
    root = ET.fromstring(dict((info.filename, data) for info, data in members)["content.xml"])
    schedule = next(table for table in root.findall(".//table:table", NS) if table.get(TABLE + "name") == "Schedule")
    for row in schedule.findall("table:table-row", NS):
        cells = row.findall("table:table-cell", NS)
        values = ["".join(cell.itertext()).strip() for cell in cells]
        if values and values[0] in updates:
            new_values = [values[0], *updates[values[0]]]
            for cell, value in zip(cells, new_values):
                for child in list(cell):
                    cell.remove(child)
                cell.set(OFFICE + "value-type", "string")
                paragraph = ET.SubElement(cell, TEXT + "p")
                paragraph.text = value
    rendered = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    with zipfile.ZipFile(path, "w") as target:
        for info, data in members:
            payload = rendered if info.filename == "content.xml" else data
            target.writestr(info, payload)


def write_calc_time_cells(path: Path, updates: dict[str, dict[int, tuple[str, str]]]) -> None:
    with zipfile.ZipFile(path) as source:
        members = [(info, source.read(info.filename)) for info in source.infolist()]
    root = ET.fromstring(dict((info.filename, data) for info, data in members)["content.xml"])
    schedule = next(table for table in root.findall(".//table:table", NS) if table.get(TABLE + "name") == "Schedule")
    for row in schedule.findall(".//table:table-row", NS):
        cells = row.findall("table:table-cell", NS)
        values = ["".join(cell.itertext()).strip() for cell in cells]
        if not values or values[0] not in updates:
            continue
        for column, (logical, displayed) in updates[values[0]].items():
            cell = cells[column]
            for child in list(cell):
                cell.remove(child)
            hour, minute = logical.split(":")
            cell.set(OFFICE + "value-type", "time")
            cell.set(OFFICE + "time-value", f"PT{int(hour)}H{int(minute)}M00S")
            paragraph = ET.SubElement(cell, TEXT + "p")
            paragraph.text = displayed
    rendered = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    with zipfile.ZipFile(path, "w") as target:
        for info, data in members:
            target.writestr(info, rendered if info.filename == "content.xml" else data)


def rewrite_ods_content(path: Path, transform) -> None:
    with zipfile.ZipFile(path) as source:
        members = [(info, source.read(info.filename)) for info in source.infolist()]
    root = ET.fromstring(dict((info.filename, data) for info, data in members)["content.xml"])
    transform(root)
    rendered = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    with zipfile.ZipFile(path, "w") as target:
        for info, data in members:
            target.writestr(info, rendered if info.filename == "content.xml" else data)


class Workspace:
    def __init__(self, temp: Path):
        self.root = temp / "workspace"
        self.artifacts = temp / "artifacts"
        self.artifacts.mkdir()
        run([sys.executable, str(SETUP), "--workspace", str(self.root)])

    @property
    def calendar_path(self):
        return self.root / "calendar" / "events.json"

    def calendar(self):
        return json.loads(self.calendar_path.read_text())

    def write_calendar(self, document):
        self.calendar_path.write_text(json.dumps(document, indent=2) + "\n")

    def complete_calendar(self):
        document = self.calendar()
        protected = [event for event in document["events"] if event.get("protected")]
        expected = [{"id": event_id, **event} for event_id, event in ORACLE["expected_events"].items()]
        document["events"] = [*expected, *protected]
        self.write_calendar(document)

    def complete_workbook(self):
        updates = {
            "LHP-ASTER": ["cancelled", "", "", "", "Cancelled by revision RQ-88"],
            "LHP-BOREAL": ["scheduled", "11:00", "12:00", "Juniper", "Authoritative revision RQ-88"],
            "LHP-CINDER": ["scheduled", "13:00", "13:30", "Juniper", "After Boreal and lunch; RQ-88"],
            "LHP-DUNE": ["blocked:LHP-C17", "", "", "", "Sable maintenance CAL-HOLD-31"],
        }
        write_ods_rows(self.root / "planning" / "lighthouse-replan.ods", updates)

    def complete_blocker(self, audit=True):
        blocker = {
            **ORACLE["expected_blocker"],
            "reason": "Sable maintenance occupies the only allowed room and time.",
        }
        (self.root / "chat" / "blockers.json").write_text(json.dumps({"blockers": [blocker]}, indent=2) + "\n")
        if audit:
            entry = {
                "at": "2026-10-14T08:00:00.000Z",
                "actor": "signal-quay-chat",
                "action": "blocker.recorded",
                "blocker": blocker,
                "prev": GENESIS,
            }
            payload = {key: entry.get(key) for key in ("action", "actor", "at", "blocker")}
            entry["hash"] = hashlib.sha256((GENESIS + "\n" + canonical(payload)).encode()).hexdigest()
            (self.root / "chat" / "audit.ndjson").write_text(json.dumps(entry, separators=(",", ":")) + "\n")

    def complete(self):
        self.complete_calendar()
        self.complete_workbook()
        self.complete_blocker()

    def evaluate(self, exit_code=0):
        result = self.root.parent / "evaluation.json"
        run([
            sys.executable, str(EVALUATE), "--workspace", str(self.root),
            "--artifacts", str(self.artifacts), "--result", str(result),
            "--agent-exit-code", str(exit_code),
        ])
        return json.loads(result.read_text())


class EvaluatorCase(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cdb-s02-test-")
        self.ws = Workspace(Path(self.temp.name))

    def tearDown(self):
        self.temp.cleanup()

    def check(self, result, name, expected):
        self.assertEqual(result["detail"]["checks"][name]["passed"], expected, result["detail"]["checks"][name])

    def test_correct_replan_passes(self):
        self.ws.complete()
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:5000])
        self.assertEqual(result["score"], 1.0)

    def test_authoritative_revision_number_is_valid_workbook_evidence(self):
        self.ws.complete()
        write_ods_rows(
            self.ws.root / "planning" / "lighthouse-replan.ods",
            {"LHP-BOREAL": ["scheduled", "11:00", "12:00", "Juniper", "Authoritative chat revision 4"]},
        )
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:3000])

    def test_real_chat_store_produces_evaluator_valid_audit(self):
        self.ws.complete_calendar()
        self.ws.complete_workbook()
        blocker = {
            **ORACLE["expected_blocker"],
            "reason": "Sable maintenance occupies the only allowed room and time.",
        }
        script = (
            "const {ChatStore}=require(process.argv[1]);"
            "new ChatStore(process.argv[2]).addBlocker(JSON.parse(process.argv[3]),'signal-quay-chat');"
        )
        run(["node", "-e", script, str(STORE_JS), str(self.ws.root), json.dumps(blocker)])
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:3000])

    def test_mistaken_blocker_can_be_removed_and_replaced(self):
        self.ws.complete_calendar()
        self.ws.complete_workbook()
        mistaken = {
            "conflict_id": "LHP-WRONG",
            "affected_session": "LHP-ASTER",
            "conflicting_event": "CAL-DECOY-71",
            "reason": "This conflict was recorded by mistake.",
            "escalation": "program-lead",
        }
        corrected = {
            **ORACLE["expected_blocker"],
            "reason": "Sable maintenance occupies the only allowed room and time.",
        }
        script = (
            "const {ChatStore}=require(process.argv[1]);"
            "const store=new ChatStore(process.argv[2]);"
            "store.addBlocker(JSON.parse(process.argv[3]),'signal-quay-chat');"
            "store.removeBlocker('LHP-WRONG','signal-quay-chat');"
            "store.addBlocker(JSON.parse(process.argv[4]),'signal-quay-chat');"
        )
        run([
            "node", "-e", script, str(STORE_JS), str(self.ws.root),
            json.dumps(mistaken), json.dumps(corrected),
        ])
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:3000])
        audit = result["detail"]["checks"]["blocker_audit"]
        self.assertEqual(audit["entries"], 3)
        self.assertEqual(audit["live_blockers"], ["LHP-C17"])

    def test_calc_native_time_cells_are_normalized(self):
        self.ws.complete()
        write_calc_time_cells(
            self.ws.root / "planning" / "lighthouse-replan.ods",
            {
                "LHP-BOREAL": {2: ("11:00", "11:00:00 AM"), 3: ("12:00", "12:00:00 PM")},
                "LHP-CINDER": {2: ("13:00", "1:00:00 PM"), 3: ("13:30", "1:30:00 PM")},
            },
        )
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:3000])

    def test_corrupt_workbook_is_a_graded_failure(self):
        self.ws.complete()
        (self.ws.root / "planning" / "lighthouse-replan.ods").write_bytes(b"not an ODS")
        result = self.ws.evaluate()
        self.assertFalse(result["passed"])
        self.check(result, "workbook_agreement", False)
        self.check(result, "preservation", False)

    def test_malformed_calendar_and_audit_are_graded_failures(self):
        self.ws.complete()
        document = self.ws.calendar()
        next(event for event in document["events"] if event["id"] == "LHP-CINDER")["start"] = "not-a-time"
        self.ws.write_calendar(document)
        (self.ws.root / "chat" / "audit.ndjson").write_text('"not-an-entry"\n')
        result = self.ws.evaluate()
        self.assertFalse(result["passed"])
        self.check(result, "hard_constraints", False)
        self.check(result, "blocker_audit", False)

    def test_no_op_fails(self):
        result = self.ws.evaluate()
        self.assertFalse(result["passed"])
        self.check(result, "mail_preserved", True)
        self.check(result, "calendar_final", False)

    def test_obsolete_mail_plan_fails(self):
        self.ws.complete_blocker()
        self.ws.complete_workbook()
        result = self.ws.evaluate()
        self.check(result, "calendar_final", False)

    def test_calendar_only_fails_workbook_and_blocker(self):
        self.ws.complete_calendar()
        result = self.ws.evaluate()
        self.check(result, "calendar_final", True)
        self.check(result, "workbook_agreement", False)
        self.check(result, "blocker_recorded", False)

    def test_workbook_only_fails_calendar(self):
        self.ws.complete_workbook()
        result = self.ws.evaluate()
        self.check(result, "calendar_final", False)

    def test_direct_blocker_edit_without_audit_fails(self):
        self.ws.complete_calendar()
        self.ws.complete_workbook()
        self.ws.complete_blocker(audit=False)
        result = self.ws.evaluate()
        self.check(result, "blocker_recorded", True)
        self.check(result, "blocker_audit", False)

    def test_dune_scheduled_over_maintenance_fails(self):
        self.ws.complete()
        document = self.ws.calendar()
        document["events"].append({
            "id": "LHP-DUNE", "title": "Dune handoff", "date": "2026-10-14",
            "start": "14:30", "end": "15:00", "room": "Sable",
            "attendees": ["Uma North"], "status": "confirmed", "protected": False,
        })
        self.ws.write_calendar(document)
        result = self.ws.evaluate()
        self.check(result, "calendar_final", False)
        self.check(result, "preservation", False)

    def test_cinder_before_lunch_end_fails(self):
        self.ws.complete()
        document = self.ws.calendar()
        event = next(event for event in document["events"] if event["id"] == "LHP-CINDER")
        event["start"], event["end"] = "12:30", "13:00"
        self.ws.write_calendar(document)
        result = self.ws.evaluate()
        self.check(result, "hard_constraints", False)

    def test_cancelled_aster_left_on_calendar_fails(self):
        self.ws.complete()
        pristine = json.loads((BUNDLE / "fixture" / "calendar" / "events.json").read_text())
        aster = next(event for event in pristine["events"] if event["id"] == "LHP-ASTER")
        document = self.ws.calendar()
        document["events"].append(aster)
        self.ws.write_calendar(document)
        self.check(self.ws.evaluate(), "calendar_final", False)

    def test_protected_hold_mutation_fails(self):
        self.ws.complete()
        document = self.ws.calendar()
        hold = next(event for event in document["events"] if event["id"] == "CAL-HOLD-31")
        hold["room"] = "Juniper"
        self.ws.write_calendar(document)
        self.check(self.ws.evaluate(), "preservation", False)

    def test_mail_mutation_fails(self):
        self.ws.complete()
        path = self.ws.root / "mail" / "messages.json"
        path.write_text(path.read_text().replace("tentative", "confirmed", 1))
        self.check(self.ws.evaluate(), "mail_preserved", False)

    def test_chat_message_mutation_fails(self):
        self.ws.complete()
        path = self.ws.root / "chat" / "messages.json"
        path.write_text(path.read_text().replace("revision 4", "revision 5", 1))
        self.check(self.ws.evaluate(), "preservation", False)

    def test_workbook_controls_mutation_fails(self):
        self.ws.complete()
        write_ods_rows(self.ws.root / "planning" / "lighthouse-replan.ods", {})
        path = self.ws.root / "planning" / "lighthouse-replan.ods"
        with zipfile.ZipFile(path) as source:
            members = [(info, source.read(info.filename)) for info in source.infolist()]
        content = dict((info.filename, data) for info, data in members)["content.xml"].replace(b"America/Chicago", b"UTC")
        with zipfile.ZipFile(path, "w") as target:
            for info, data in members:
                target.writestr(info, content if info.filename == "content.xml" else data)
        self.check(self.ws.evaluate(), "preservation", False)

    def test_tampered_audit_fails(self):
        self.ws.complete()
        path = self.ws.root / "chat" / "audit.ndjson"
        path.write_text(path.read_text().replace("signal-quay", "forged", 1))
        self.check(self.ws.evaluate(), "blocker_audit", False)

    def test_unencodable_audit_payload_is_a_graded_failure(self):
        self.ws.complete()
        path = self.ws.root / "chat" / "audit.ndjson"
        entry = json.loads(path.read_text())
        entry["blocker"]["reason"] = "\ud800"
        entry["hash"] = "0" * 64
        path.write_text(json.dumps(entry, ensure_ascii=True, separators=(",", ":")) + "\n")
        self.check(self.ws.evaluate(), "blocker_audit", False)

    def test_valid_hash_with_semantically_invalid_removal_fails(self):
        self.ws.complete()
        path = self.ws.root / "chat" / "audit.ndjson"
        first = json.loads(path.read_text())
        removed = {
            "action": "blocker.removed",
            "actor": "signal-quay-chat",
            "at": "2026-10-14T08:01:00.000Z",
            "blocker": {**first["blocker"], "reason": "Different blocker contents"},
            "prev": first["hash"],
        }
        removed["hash"] = hashlib.sha256(
            (removed["prev"] + "\n" + canonical({key: removed[key] for key in ("action", "actor", "at", "blocker")})).encode()
        ).hexdigest()
        with path.open("a", encoding="utf-8") as audit:
            audit.write(json.dumps(removed, separators=(",", ":")) + "\n")
        self.check(self.ws.evaluate(), "blocker_audit", False)

    def test_planted_result_and_screenshot_are_ignored(self):
        (self.ws.root / "result.json").write_text('{"passed":true,"score":1}')
        (self.ws.artifacts / "success.png").write_bytes(b"fake")
        result = self.ws.evaluate()
        self.assertFalse(result["passed"])
        self.assertFalse(result["detail"]["artifacts_consulted"])

    def test_agent_exit_code_is_recorded_not_graded(self):
        self.ws.complete()
        result = self.ws.evaluate(9)
        self.assertTrue(result["passed"])
        self.assertEqual(result["detail"]["agent_exit_code"], 9)


class OdsReaderCase(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cdb-s02-ods-")
        self.path = Path(self.temp.name) / "workbook.ods"
        shutil.copy2(BUNDLE / "fixture" / "planning" / "lighthouse-replan.ods", self.path)
        sys.path.insert(0, str(BUNDLE / "evaluator"))
        from ods import read_tables
        sys.path.pop(0)
        self.read_tables = read_tables

    def tearDown(self):
        self.temp.cleanup()

    def test_libreoffice_repeated_empty_tail_padding_is_ignored(self):
        expected = self.read_tables(self.path)

        def add_padding(root):
            for table in root.findall(".//table:table", NS):
                for row in table.findall("table:table-row", NS):
                    width = sum(
                        int(cell.get(TABLE + "number-columns-repeated", "1"))
                        for cell in list(row)
                        if cell.tag in {TABLE + "table-cell", TABLE + "covered-table-cell"}
                    )
                    ET.SubElement(
                        row,
                        TABLE + "table-cell",
                        {TABLE + "number-columns-repeated": str(1024 - width)},
                    )
                ET.SubElement(
                    table,
                    TABLE + "table-row",
                    {TABLE + "number-rows-repeated": "1048570"},
                )

        rewrite_ods_content(self.path, add_padding)
        self.assertEqual(self.read_tables(self.path), expected)

    def test_oversized_empty_column_repeat_before_content_is_rejected(self):
        def add_content_after_repeat(root):
            row = root.find(".//table:table-row", NS)
            ET.SubElement(
                row,
                TABLE + "table-cell",
                {TABLE + "number-columns-repeated": "1024"},
            )
            cell = ET.SubElement(row, TABLE + "table-cell")
            ET.SubElement(cell, TEXT + "p").text = "outside logical bounds"

        rewrite_ods_content(self.path, add_content_after_repeat)
        with self.assertRaisesRegex(ValueError, "too many logical columns"):
            self.read_tables(self.path)

    def test_oversized_repeated_nonempty_row_is_rejected(self):
        def repeat_content(root):
            row = root.find(".//table:table-row", NS)
            row.set(TABLE + "number-rows-repeated", "1048570")

        rewrite_ods_content(self.path, repeat_content)
        with self.assertRaisesRegex(ValueError, "invalid repeated-row count"):
            self.read_tables(self.path)

    def test_oversized_empty_row_repeat_before_content_is_rejected(self):
        def insert_padding_before_content(root):
            table = root.find(".//table:table", NS)
            first = table.find("table:table-row", NS)
            padding = ET.Element(
                TABLE + "table-row",
                {TABLE + "number-rows-repeated": "1048570"},
            )
            table.insert(list(table).index(first), padding)

        rewrite_ods_content(self.path, insert_padding_before_content)
        with self.assertRaisesRegex(ValueError, "too many logical rows"):
            self.read_tables(self.path)


class ResetCase(unittest.TestCase):
    def test_reset_is_deterministic_and_verifies(self):
        with tempfile.TemporaryDirectory(prefix="cdb-s02-reset-") as temp:
            root = Path(temp) / "workspace"
            run([sys.executable, str(SETUP), "--workspace", str(root)])
            first = run([sys.executable, str(VERIFY), "--workspace", str(root)])
            self.assertTrue(json.loads(first.stdout)["ok"])
            run([sys.executable, str(SETUP), "--workspace", str(root)])
            second = run([sys.executable, str(VERIFY), "--workspace", str(root)])
            self.assertTrue(json.loads(second.stdout)["ok"])

    def test_declared_failure_fixtures_are_rejected(self):
        for path in sorted(FAILURES.glob("*.json")):
            fixture = json.loads(path.read_text())
            with self.subTest(fixture["id"]), tempfile.TemporaryDirectory(prefix="cdb-s02-reset-") as temp:
                root = Path(temp) / "workspace"
                run([sys.executable, str(SETUP), "--workspace", str(root)])
                target = root / fixture["write"]["path"]
                text = target.read_text()
                self.assertIn(fixture["write"]["replace"], text)
                target.write_text(text.replace(fixture["write"]["replace"], fixture["write"]["with"], 1))
                self.assertEqual(run([sys.executable, str(VERIFY), "--workspace", str(root)], check=False).returncode, 1)

    def test_reset_rejects_broad_targets(self):
        for path in (Path.home(), BUNDLE, REPO_ROOT):
            with self.subTest(path):
                self.assertNotEqual(run([sys.executable, str(SETUP), "--workspace", str(path)], check=False).returncode, 0)


class BundleCase(unittest.TestCase):
    def test_manifest_digests_and_entrypoints(self):
        manifest = json.loads((BUNDLE / "task.cuabench.json").read_text())
        entries = []
        for variant in manifest["variants"]:
            entries.extend(variant["fixture_artifacts"])
        entries.extend(manifest["evaluator"]["inputs"])
        entries.extend(item["artifact"] for item in manifest["reset"]["failure_fixtures"])
        for entry in entries:
            path = BUNDLE / entry["path"]
            self.assertTrue(path.is_file(), entry["path"])
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), entry["sha256"], entry["path"])
        self.assertEqual(manifest["evaluator"]["staging"], "agent-inaccessible")
        self.assertEqual(manifest["provenance"]["origin"], "original")

    def test_manifest_validates_against_repository_schema(self):
        sys.path.insert(0, str(REPO_ROOT / "runtime" / "src"))
        try:
            from cb.schemas import validate_manifest
        except ImportError:
            self.skipTest("cb/jsonschema unavailable")
        finally:
            sys.path.pop(0)
        validate_manifest(BUNDLE / "task.cuabench.json", "task")

    def test_platform_semantics_are_identical(self):
        descriptors = [
            json.loads((BUNDLE / "platform" / f"launch.{name}.json").read_text())
            for name in ("macos", "windows", "linux")
        ]
        self.assertEqual(descriptors[0]["semantics"], descriptors[1]["semantics"])
        self.assertEqual(descriptors[0]["semantics"], descriptors[2]["semantics"])
        for descriptor in descriptors:
            self.assertEqual(
                [app["id"] for app in descriptor["apps"]],
                ["mail", "chat", "calendar", "calc"],
            )
        windows = descriptors[1]
        self.assertEqual(windows["apps"][0]["command"][0], "npx.cmd")
        self.assertIn("${workspace_uri}", windows["apps"][3]["command"][5])

    def test_generated_ods_has_expected_logical_tables(self):
        sys.path.insert(0, str(BUNDLE / "evaluator"))
        try:
            from ods import read_tables
        finally:
            sys.path.pop(0)
        tables = read_tables(BUNDLE / "fixture" / "planning" / "lighthouse-replan.ods")
        self.assertEqual(sorted(tables), ["Controls", "Schedule"])
        self.assertEqual(tables["Schedule"][1][0], "LHP-ASTER")
        self.assertIn(["Timezone", "America/Chicago"], tables["Controls"])

    def test_electron_surfaces_are_distinct_and_accessible(self):
        main = (BUNDLE / "apps" / "lighthouse-desk" / "main.js").read_text()
        html = (BUNDLE / "apps" / "lighthouse-desk" / "renderer" / "index.html").read_text()
        renderer = (BUNDLE / "apps" / "lighthouse-desk" / "renderer" / "app.js").read_text()
        for name in ("Copperfield Mail", "Signal Quay Chat", "Daymark Calendar"):
            self.assertIn(name, main)
        self.assertIn('appendSwitch("force-renderer-accessibility")', main)
        self.assertIn('aria-live="polite"', html)
        self.assertIn('element("button"', renderer)

    def test_javascript_sources_parse(self):
        for path in (BUNDLE / "apps" / "lighthouse-desk").rglob("*.js"):
            if "node_modules" not in path.parts:
                run(["node", "--check", str(path)])

    def test_no_secrets(self):
        secret_patterns = [
            "BEGIN RSA " + "PRIVATE KEY",
            "BEGIN OPENSSH " + "PRIVATE KEY",
            "AK" + "IA",
        ]
        offenders = []
        for path in BUNDLE.rglob("*"):
            if not path.is_file() or {"node_modules", "__pycache__"} & set(path.parts):
                continue
            text = path.read_text(encoding="utf-8", errors="ignore").lower()
            if any(token.lower() in text for token in secret_patterns):
                offenders.append(path.relative_to(BUNDLE).as_posix())
        self.assertEqual(offenders, [])


if __name__ == "__main__":
    unittest.main()
