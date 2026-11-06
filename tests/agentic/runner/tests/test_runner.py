"""Unit tests for the prompt test runner: no model, no machine.

python3 -m pytest tests/agentic/runner/tests -q
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from runner import checks, spec, trajectory  # noqa: E402

GOOD = """\
name: demo
os: [linux]
setup:
  build: spaces-app
  account: canary
  hosts:
    - {id: 1, role: app}
    - {id: 2, role: computer, joined: true, terminal: Second computer}
prompt: Turn {{host.2.name}} off and check the app shows it offline.
budget: {steps: 10, minutes: 5}
expect:
  - ui: {app: Cua Spaces, row: {has: "{{host.2.name}}", and: [Offline]}, after_event: "power_off:2", within_s: 60}
  - cli: {host: 2, run: cua host status --json, jq: .online}
  - relay: {get: /v1/machines, jq: '.machines | length >= 0'}
  - no_dialogs: {titles: [Windows Security Alert]}
"""


def write(tmp_path: Path, text: str, name: str = "demo") -> Path:
    d = tmp_path / "area"
    d.mkdir(exist_ok=True)
    p = d / f"{name}.yaml"
    p.write_text(text)
    return p


def test_every_committed_spec_is_valid():
    paths = spec.discover()
    assert len(paths) >= 5
    for p in paths:
        spec.load(p)


def test_good_spec_loads(tmp_path):
    s = spec.load(write(tmp_path, GOOD))
    assert s.key == "area/demo"
    assert s.host(2).service == "process" and s.host(1).service == "real"
    assert [c["type"] for c in s.expect] == ["ui", "cli", "relay", "no_dialogs"]


@pytest.mark.parametrize(
    "old,new,msg",
    [
        ("os: [linux]", "os: [beos]", "is not one of"),
        ("account: canary", "account: francesco", "only the canary"),
        (
            "  - relay: {get: /v1/machines, jq: '.machines | length >= 0'}",
            "  - relya: {get: /v1/machines, jq: .}",
            "unknown check type",
        ),
        ("run: cua host status --json", "run: cua host status", "cua ... --json"),
        ("{{host.2.name}} off", "{{host.3.name}} off", "names a host"),
        ("{{host.2.name}} off", "{{secret}} off", "unknown template variable"),
        ("budget: {steps: 10, minutes: 5}", "budget: {steps: 10}", "missing minutes"),
        ('after_event: "power_off:2"', "after_event: reboot", "power_off:N"),
        (
            "    - {id: 1, role: app}",
            "    - {id: 1, role: computer}",
            "exactly one host has role app",
        ),
    ],
)
def test_bad_specs_are_refused(tmp_path, old, new, msg):
    assert old in GOOD
    with pytest.raises(spec.SpecError, match=msg):
        spec.load(write(tmp_path, GOOD.replace(old, new)))


def test_unknown_keys_are_refused(tmp_path):
    with pytest.raises(spec.SpecError, match="unknown key"):
        spec.load(write(tmp_path, GOOD + "retries: 3\n"))


def test_file_name_must_match(tmp_path):
    with pytest.raises(spec.SpecError, match="named demo.yaml"):
        spec.load(write(tmp_path, GOOD, name="other"))


def test_render():
    assert spec.render({"a": ["x {{host.2.name}}"]}, {"host.2.name": "e2e-1-h2"}) == {
        "a": ["x e2e-1-h2"]
    }
    with pytest.raises(spec.SpecError):
        spec.render("{{run}}", {})


TREE = """\
- [0] window "Cua Spaces"
  - [4] row "e2e-1-h2"
    - [5] text "Offline"
  - [6] row "laptop"
    - [7] text "Online"
"""


def test_ui_assert():
    assert checks.ui_assert(TREE, {"contains": ["laptop"]})[0]
    assert not checks.ui_assert(TREE, {"absent": ["e2e-1-h2"]})[0]
    assert checks.ui_assert(TREE, {"absent": ["", "ghost"]})[0]
    assert checks.ui_assert(TREE, {"row": {"has": ["e2e-1-h2"], "and": ["offline"]}})[0]
    assert not checks.ui_assert(TREE, {"row": {"has": ["laptop"], "and": ["offline"]}})[0]
    # A neighbour's status never counts for this row.
    flat = '- [4] item "e2e-1-h2 Desktop"\n  - static = "e2e-1-h2"\n- [6] item "old box"\n  - static = "Offline"\n'
    assert not checks.ui_assert(flat, {"row": {"has": ["e2e-1-h2"], "and": ["offline"]}})[0]


def test_first_json_ignores_trailing_output():
    assert checks.first_json('{"online": true}\n{"other": 1}\n') == {"online": True}


def test_jq_predicate():
    assert checks.jq_ok(".a == 1", {"a": 1})[0]
    assert not checks.jq_ok(".a == 2", {"a": 1})[0]


def test_trajectory_portable_and_remap(tmp_path):
    run = tmp_path / "run"
    rec = run / "recording"
    (rec / "turn-00001").mkdir(parents=True)
    (rec / "turn-00001" / "action.json").write_text(
        json.dumps(
            {
                "tool": "click",
                "arguments": {"pid": 11, "window_id": 22, "element_token": "s1:4"},
                "timestamp": "2026-11-05T10:00:00Z",
            }
        )
    )
    (rec / "turn-00002").mkdir()
    (rec / "turn-00002" / "action.json").write_text(
        json.dumps(
            {
                "tool": "type_text",
                "arguments": {"pid": 33, "text": "K7QM-4XPB-2RHD"},
                "timestamp": "2026-11-05T10:00:03Z",
            }
        )
    )
    (run / "calls.jsonl").write_text(
        json.dumps(
            {
                "step": 1,
                "tool": "list_windows",
                "structured": {
                    "windows": [
                        {
                            "pid": 11,
                            "window_id": 22,
                            "app_name": "Cua Spaces",
                            "title": "Cua Spaces",
                        },
                        {
                            "pid": 33,
                            "window_id": 44,
                            "app_name": "xterm",
                            "title": "Second computer",
                        },
                    ]
                },
            }
        )
        + "\n"
    )
    (run / "snapshots.jsonl").write_text(
        json.dumps(
            {
                "snapshot_id": "s1",
                "pid": 11,
                "window_id": 22,
                "tree": TREE + '  - [9] text "K7QM-4XPB-2RHD"\n',
            }
        )
        + "\n"
    )
    p = trajectory.build_portable(run, rec, "prompt")
    assert p["turns"][0]["tokens"]["s1:4"]["row"] == 'row "e2e-1-h2"'
    assert p["turns"][0]["pids"] == {"11": "Cua Spaces"}
    assert not p["replayable"] and "read off the screen" in p["not_replayable_because"]
    args = trajectory._remap(
        {"pid": 11, "window_id": 22, "steps": [{"pid": 11}]}, {11: 99}, {22: 77}
    )
    assert args == {"pid": 99, "window_id": 77, "steps": [{"pid": 99}]}
    assert trajectory._replace_token(
        {"element_token": "s1:4", "x": [{"element_token": "s1:4"}]}, "s1:4", "s9:2"
    ) == {"element_token": "s9:2", "x": [{"element_token": "s9:2"}]}
