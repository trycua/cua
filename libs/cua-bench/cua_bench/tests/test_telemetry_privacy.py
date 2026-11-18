"""cua-bench telemetry never sends user-chosen names, paths or error messages.

record_event is always replaced with a recorder; nothing reaches the network.
"""

from __future__ import annotations

import json
import socket
import sys
import types

import pytest
from cua_bench.telemetry import events


@pytest.fixture
def recorded(monkeypatch):
    sent: list[tuple[str, dict]] = []

    def _blocked(*_a, **_k):
        raise AssertionError("network access attempted during telemetry test")

    monkeypatch.setattr(socket.socket, "connect", _blocked)
    monkeypatch.setattr(socket, "create_connection", _blocked)
    monkeypatch.setattr(events, "_core_is_telemetry_enabled", lambda: True)
    monkeypatch.setattr(events, "_core_record_event", lambda n, p=None: sent.append((n, p or {})))
    # Force the cua-core fallback path for the run event.
    monkeypatch.setitem(sys.modules, "cua._native", None)
    return sent


PII = "alice-secret"


def _blob(sent):
    return json.dumps(sent, default=str)


@pytest.mark.parametrize(
    "name,expected",
    [
        ("cua-bench-basic", "cua-bench-basic"),
        ("cua-bench-kicad@1.0", "cua-bench-kicad"),
        ("./datasets/cua-bench-workflows/", "cua-bench-workflows"),
        ("/Users/alice/alice-secret-dataset", "custom"),
        ("alice-secret", "custom"),
        (None, "custom"),
        ("", "custom"),
    ],
)
def test_taskset_id(name, expected):
    assert events.taskset_id(name) == expected


@pytest.mark.parametrize(
    "rate,expected",
    [
        (None, "none"),
        (0, "0"),
        (0.0, "0"),
        (0.01, "1_24"),
        (0.24, "1_24"),
        (0.25, "25_49"),
        (0.5, "50_74"),
        (0.75, "75_99"),
        (0.999, "75_99"),
        (1.0, "100"),
        (float("nan"), "none"),
    ],
)
def test_score_bucket(rate, expected):
    assert events.score_bucket(rate) == expected


@pytest.mark.parametrize(
    "n,expected",
    [
        (0, "0"),
        (1, "1"),
        (2, "2_4"),
        (4, "2_4"),
        (5, "5_9"),
        (9, "5_9"),
        (10, "10_49"),
        (49, "10_49"),
        (50, "50_99"),
        (99, "50_99"),
        (100, "gte_100"),
        (5000, "gte_100"),
    ],
)
def test_count_bucket(n, expected):
    assert events.count_bucket(n) == expected


def test_run_completed_fallback_payload(recorded):
    events.track_bench_run_completed("/home/alice/alice-secret", 0.6, 12, "ok")
    ((name, props),) = recorded
    assert name == "cua_bench_run_completed"
    assert {k: props[k] for k in ("taskset", "score_bucket", "task_count", "outcome")} == {
        "taskset": "custom",
        "score_bucket": "50_74",
        "task_count": "10_49",
        "outcome": "ok",
    }
    assert PII not in _blob(recorded)


def test_run_completed_prefers_sdk(recorded, monkeypatch):
    calls = []
    fake = types.ModuleType("cua._native")
    fake.telemetry_record_bench_run = lambda *a: calls.append(a)
    pkg = types.ModuleType("cua")
    pkg.__path__ = []  # mark as package
    pkg._native = fake
    monkeypatch.setitem(sys.modules, "cua", pkg)
    monkeypatch.setitem(sys.modules, "cua._native", fake)
    events.track_bench_run_completed("cua-bench-basic", None, 3, "cancelled")
    assert calls == [("cua-bench-basic", None, 3, "cancelled")]
    assert recorded == []


def test_run_completed_disabled(monkeypatch):
    sent = []
    monkeypatch.setattr(events, "_core_is_telemetry_enabled", lambda: False)
    monkeypatch.setattr(events, "_core_record_event", lambda n, p=None: sent.append(n))
    events.track_bench_run_completed("cua-bench-basic", 1.0, 3, "ok")
    assert sent == []


def test_failure_event_has_no_error_message(recorded):
    events.track_task_execution_failed(
        env_name=f"/Users/alice/{PII}-task",
        task_index=0,
        error_type="ValueError",
        error_message=f"could not open /Users/alice/{PII}.txt",
        stage="setup",
    )
    ((name, props),) = recorded
    assert name == "cb_task_execution_failed"
    assert "error_message" not in props
    assert props["error_type"] == "ValueError"
    assert props["env_name"] == "custom"
    assert PII not in _blob(recorded)


def test_env_name_known_vs_custom(recorded):
    events.track_task_execution_started("click-button", 0, provider_type="native", os_type="linux")
    events.track_task_execution_started(f"{PII}-dir", 0, provider_type=PII, os_type=PII)
    (_, known), (_, custom) = recorded
    assert known["env_name"] == "click-button"
    assert custom["env_name"] == "custom"
    assert custom["provider_type"] == "other"
    assert custom["os_type"] == "other"
    assert PII not in _blob(recorded)


def test_common_properties_are_coarse(recorded):
    events.track_command_invoked("run")
    ((_, props),) = recorded
    assert "os_version" not in props
    assert "timestamp" not in props
    assert props["python_version"] == f"{sys.version_info.major}.{sys.version_info.minor}"


def test_command_args_sanitized(recorded):
    events.track_command_invoked(
        "run",
        "dataset",
        {
            "agent": f"{PII}-agent",
            "model": "/Users/alice/models/alice-secret.gguf",
            "max_steps": 10,
            "on": "local",
            "runtime": PII,
            "unknown_flag": PII,
        },
    )
    args = recorded[0][1]["args"]
    assert args["agent"] == "custom"
    assert args["model"] == "custom"
    assert args["max_steps"] == 10
    assert args["on"] == "local"
    assert args["runtime"] == "other"
    assert "unknown_flag" not in args
    assert PII not in _blob(recorded)


@pytest.mark.parametrize(
    "model",
    ["http://10.0.0.1:8000/v1", "C:\\models\\x", "alice/private", "x" * 80],
)
def test_model_label_custom(model):
    assert events.model_label(model) == "custom"


def test_model_label_known():
    assert events.model_label("anthropic/claude-sonnet-4-20250514") == (
        "anthropic/claude-sonnet-4-20250514"
    )
    assert events.agent_label("cua-agent") == "cua-agent"
