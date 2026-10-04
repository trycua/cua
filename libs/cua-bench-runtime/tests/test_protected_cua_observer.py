from __future__ import annotations

import json
import hashlib
import tempfile
import unittest
from pathlib import Path, PurePosixPath
from types import SimpleNamespace

from cua_bench_runtime.adapters.protected_cua import ProtectedCuaMediatorObserver
from cua_bench_runtime.canon import digest_json
from cua_bench_runtime.errors import HarnessFailure
from cua_bench_runtime.model import EnvironmentHandle, TrialContext
from cua_bench_runtime.participation import required_fact_digests


def event(kind: str, facts: dict) -> dict:
    return {
        "provider": {"id": "trycua.cua-driver", "version": "0.20.0"},
        "capability_class": ("pointer_input" if kind == "act" else "accessibility_observation"),
        "target": {
            "application_id": "application.example-desk",
            "surface_id": "surface.record.item-1042",
            "platform_application_id": "com.github.Electron",
            "process_id": 41,
            "window_id": "73",
        },
        "kind": kind,
        "correlation_id": "cdb-mediator:synthetic-task.v1:41:73:ITEM-1042",
        "fact_digests": required_fact_digests(facts),
    }


class ProtectedObserverTests(unittest.TestCase):
    def context(self, root: Path) -> TrialContext:
        return TrialContext(
            trial_id="trial-alpha",
            task={"id": "synthetic-task.v1"},
            task_path=root / "task.json",
            config={},
            trial_dir=root,
            artifacts=root / "artifacts",
            harness_workspace=root / "harness",
            emit=lambda _kind, _body: None,
        )

    @staticmethod
    def sealed_environment(root: Path, *, off_target: bool = False, evidence_complete: bool = True):
        protected = root / "protected"
        protected.mkdir()
        log = protected / "mediator.ndjson"
        events = [
            event("observe", {"record_id": "ITEM-1042"}),
            event("act", {"record_id": "ITEM-1042", "control_label": "commit"}),
            event(
                "readback",
                {
                    "record_id": "ITEM-1042",
                    "state": "active",
                    "result_code": "updated",
                },
            ),
        ]
        records = []
        previous = "0" * 64
        for sequence, fields in enumerate(
            [
                *({"kind": "event", "event": item} for item in events),
                {
                    "kind": "seal",
                    "fatal": False,
                    "evidence_complete": evidence_complete,
                    "certifying": not off_target and evidence_complete,
                    "off_target_activity": off_target,
                },
            ],
            1,
        ):
            record_body = {
                "seq": sequence,
                "prev": previous,
                "attempt": "trial-alpha",
                "task": "synthetic-task.v1",
                **fields,
            }
            record_hash = hashlib.sha256(
                json.dumps(
                    record_body,
                    sort_keys=True,
                    separators=(",", ":"),
                    ensure_ascii=False,
                ).encode("utf-8")
            ).hexdigest()
            records.append({**record_body, "hash": record_hash})
            previous = record_hash
        log.write_text(
            "".join(
                json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
                for record in records
            ),
            encoding="utf-8",
        )
        body = {
            "attempt": "trial-alpha",
            "events": events,
            "event_log_sha256": hashlib.sha256(log.read_bytes()).hexdigest(),
            "event_log_tail": records[-1]["hash"],
            "records": len(records),
            "worker_uid": 501,
            "agent_uid": 502,
            "mediator_sha256": "b" * 64,
            "transport_integrity": True,
            "evidence_complete": evidence_complete,
            "off_target_activity": off_target,
            "frontend_device": 1,
            "frontend_inode": 2,
            "backend_device": 3,
            "backend_inode": 4,
            "backend_parent_device": 5,
            "backend_parent_inode": 6,
            "target_pid": 41,
        }
        report = {
            **body,
            "report_digest": digest_json(body).removeprefix("sha256:"),
        }
        report_path = protected / "participation.sealed.json"
        report_path.write_text(
            json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        return SimpleNamespace(
            protected_report={"verb": "mediator-stop-and-seal", **report},
            protected_report_path=None,
            protected_log_path=log,
            guest_root=PurePosixPath("/Users/Shared/cdb-attempts/trial-alpha"),
        )

    def test_accepts_only_matching_stopped_disk_seal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            environment = self.sealed_environment(root)
            observer = ProtectedCuaMediatorObserver(environment)
            handle = EnvironmentHandle(
                "lume-macos-certifying",
                root,
                {
                    "mediator_enforcer": "guest-root-cdb-helper",
                    "vm_stopped_before_collection": True,
                    "protected_collection_read_only": True,
                },
            )
            observer.start(self.context(root), handle, ())
            observed = observer.finish(self.context(root), handle, None)
            self.assertEqual(observed.trust, "certifying")
            self.assertEqual(len(observed.events), 3)

    def test_off_target_activity_is_never_certifying(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            environment = self.sealed_environment(root, off_target=True)
            observer = ProtectedCuaMediatorObserver(environment)
            handle = EnvironmentHandle(
                "lume-macos-certifying",
                root,
                {
                    "mediator_enforcer": "guest-root-cdb-helper",
                    "vm_stopped_before_collection": True,
                    "protected_collection_read_only": True,
                },
            )
            observer.start(self.context(root), handle, ())
            observed = observer.finish(self.context(root), handle, None)
            self.assertEqual(observed.trust, "non_certifying")
            self.assertEqual(len(observed.events), 3)
            self.assertTrue(environment.protected_report["evidence_complete"])

    def test_incomplete_evidence_is_never_certifying(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            environment = self.sealed_environment(root, evidence_complete=False)
            observer = ProtectedCuaMediatorObserver(environment)
            handle = EnvironmentHandle(
                "lume-macos-certifying",
                root,
                {
                    "mediator_enforcer": "guest-root-cdb-helper",
                    "vm_stopped_before_collection": True,
                    "protected_collection_read_only": True,
                },
            )
            observer.start(self.context(root), handle, ())
            observed = observer.finish(self.context(root), handle, None)
            self.assertEqual(observed.trust, "non_certifying")
            self.assertEqual(len(observed.events), 3)

    def test_rejects_live_disk_mismatch_and_unstopped_collection(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "participation.sealed.json"
            path.write_text("{}\n", encoding="utf-8")
            environment = SimpleNamespace(
                protected_report={"verb": "mediator-stop-and-seal", "attempt": "wrong"},
                protected_report_path=path,
                protected_log_path=path,
                guest_root=PurePosixPath("/Users/Shared/cdb-attempts/trial-alpha"),
            )
            observer = ProtectedCuaMediatorObserver(environment)
            handle = EnvironmentHandle(
                "lume-macos-certifying",
                root,
                {
                    "mediator_enforcer": "guest-root-cdb-helper",
                    "vm_stopped_before_collection": False,
                    "protected_collection_read_only": False,
                },
            )
            observer.start(self.context(root), handle, ())
            with self.assertRaises(HarnessFailure):
                observer.finish(self.context(root), handle, None)


if __name__ == "__main__":
    unittest.main()
