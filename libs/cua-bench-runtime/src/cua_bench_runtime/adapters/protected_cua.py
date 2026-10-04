"""Certifying observer backed by a sealed protected guest mediator."""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from cua_bench_runtime.adapters import ObserverAdapter
from cua_bench_runtime.canon import digest_file, digest_json
from cua_bench_runtime.errors import HarnessFailure
from cua_bench_runtime.model import AgentOutcome, EnvironmentHandle, ObserverReport, TrialContext
from cua_bench_runtime.participation import normalize_event

_HEX = re.compile(r"^[a-f0-9]{64}$")
_MAX_LOG_BYTES = 16 * 1024 * 1024


def _load_stopped_chain(path: Path, attempt: str, task_id: str) -> tuple[list[dict[str, Any]], str]:
    if not path.is_file() or path.is_symlink():
        raise HarnessFailure("protected mediator stopped-disk log is unavailable")
    content = path.read_bytes()
    if not content or len(content) > _MAX_LOG_BYTES:
        raise HarnessFailure("protected mediator stopped-disk log size is invalid")
    records: list[dict[str, Any]] = []
    previous = "0" * 64
    for sequence, line in enumerate(content.splitlines(), 1):
        if len(line) > 1024 * 1024:
            raise HarnessFailure("protected mediator stopped-disk record is too large")
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise HarnessFailure("protected mediator stopped-disk log is invalid") from error
        if (
            not isinstance(record, dict)
            or record.get("seq") != sequence
            or record.get("prev") != previous
            or record.get("attempt") != attempt
            or record.get("task") != task_id
        ):
            raise HarnessFailure("protected mediator stopped-disk chain is discontinuous")
        claimed = record.get("hash")
        body = {key: value for key, value in record.items() if key != "hash"}
        actual = hashlib.sha256(
            json.dumps(
                body,
                sort_keys=True,
                separators=(",", ":"),
                ensure_ascii=False,
            ).encode("utf-8")
        ).hexdigest()
        if claimed != actual:
            raise HarnessFailure("protected mediator stopped-disk hash mismatch")
        previous = claimed
        records.append(record)
    seal = records[-1]
    if (
        seal.get("kind") != "seal"
        or not isinstance(seal.get("fatal"), bool)
        or not isinstance(seal.get("certifying"), bool)
        or not isinstance(seal.get("off_target_activity"), bool)
    ):
        raise HarnessFailure("protected mediator stopped-disk log did not seal")
    return records, hashlib.sha256(content).hexdigest()


class ProtectedCuaMediatorObserver(ObserverAdapter):
    """Consume only stopped-disk evidence that matches the live root seal."""

    name = "protected-cua-driver-mediator"

    def __init__(self, environment: Any) -> None:
        self.environment = environment
        self.started = False

    def start(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        requirements: tuple[Mapping[str, Any], ...],
    ) -> None:
        del context, requirements
        if handle.kind != "lume-macos-certifying":
            raise HarnessFailure("protected observer requires the certifying Lume adapter")
        if handle.facts.get("mediator_enforcer") != "guest-root-cdb-helper":
            raise HarnessFailure("protected mediator enforcement is unavailable")
        self.started = True

    def finish(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome | None,
    ) -> ObserverReport:
        del outcome
        if not self.started:
            raise HarnessFailure("protected observer did not start")
        report = self.environment.protected_report
        report_path = self.environment.protected_report_path
        log_path = self.environment.protected_log_path
        if not isinstance(report, Mapping) or not isinstance(log_path, Path):
            raise HarnessFailure("protected mediator report was not collected")
        if report.get("attempt") != self.environment.guest_root.name:
            raise HarnessFailure("protected mediator attempt binding mismatch")
        live_report = {key: value for key, value in report.items() if key != "verb"}
        records, stopped_log_digest = _load_stopped_chain(
            log_path,
            self.environment.guest_root.name,
            str(context.task["id"]),
        )
        seal = records[-1]
        stopped_events = [record["event"] for record in records if record.get("kind") == "event"]
        seal_evidence_complete = seal.get("evidence_complete")
        if not isinstance(seal_evidence_complete, bool):
            raise HarnessFailure("protected mediator seal evidence completeness is invalid")
        expected_from_chain = {
            "events": stopped_events,
            "event_log_sha256": stopped_log_digest,
            "event_log_tail": seal["hash"],
            "records": len(records),
            "transport_integrity": seal["fatal"] is False,
            "evidence_complete": seal_evidence_complete,
            "off_target_activity": seal["off_target_activity"],
        }
        if any(live_report.get(key) != value for key, value in expected_from_chain.items()):
            raise HarnessFailure("live report and stopped-disk mediator chain differ")
        if isinstance(report_path, Path):
            if not report_path.is_file() or report_path.is_symlink():
                raise HarnessFailure("protected mediator stopped-disk report is unavailable")
            try:
                stopped_report = json.loads(report_path.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, json.JSONDecodeError) as error:
                raise HarnessFailure("protected mediator stopped-disk report is invalid") from error
            if stopped_report != live_report:
                raise HarnessFailure("live and stopped-disk mediator reports differ")
        report_body = {key: value for key, value in live_report.items() if key != "report_digest"}
        if not _HEX.fullmatch(str(live_report.get("report_digest"))) or live_report[
            "report_digest"
        ] != digest_json(report_body).removeprefix("sha256:"):
            raise HarnessFailure("protected mediator report digest mismatch")
        if digest_file(log_path).removeprefix("sha256:") != stopped_log_digest:
            raise HarnessFailure("protected mediator event log digest mismatch")
        events = live_report.get("events")
        if not isinstance(events, list):
            raise HarnessFailure("protected mediator events are invalid")
        normalized = tuple(normalize_event(event) for event in events)
        if handle.facts.get("vm_stopped_before_collection") is not True:
            raise HarnessFailure("protected evidence was not collected from a stopped VM")
        if handle.facts.get("protected_collection_read_only") is not True:
            raise HarnessFailure("protected evidence collection was not read-only")
        if live_report.get("transport_integrity") is not True:
            return ObserverReport(
                name=self.name,
                trust="unavailable",
                detail="protected mediator transport did not remain continuous",
            )
        if live_report.get("off_target_activity") is True:
            return ObserverReport(
                name=self.name,
                trust="non_certifying",
                events=normalized,
                detail="protected mediator observed GUI activity outside the pinned task app",
            )
        if live_report.get("evidence_complete") is not True:
            return ObserverReport(
                name=self.name,
                trust="non_certifying",
                events=normalized,
                detail="protected mediator evidence sequence is incomplete",
            )
        return ObserverReport(name=self.name, trust="certifying", events=normalized)
