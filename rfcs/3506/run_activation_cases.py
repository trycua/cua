#!/usr/bin/python
"""Real activation/refusal cases. No keyboard/pointer transport is imported."""
import argparse
import json
from pathlib import Path
import subprocess
import time

import dbus
from dbus.mainloop.glib import DBusGMainLoop

from activation_probe import exact_record, observe, trusted_snapshot

parser = argparse.ArgumentParser()
parser.add_argument("--directory", type=Path, required=True)
args = parser.parse_args()
DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
fixture_owner = json.loads((args.directory / "fixture-owner.json").read_text())
fixture = dbus.Interface(bus.get_object(fixture_owner["owner"], "/Fixture"),
                         "org.cua.ProofFixture")
pid = fixture_owner["pid"]
output = (args.directory / "activation-cases.jsonl").open("x", buffering=1)


def state():
    return json.loads(str(fixture.GetState()))


def selected():
    deadline = time.monotonic() + 2
    while True:
        owner, _, snapshot = trusted_snapshot(bus)
        matching = [w for w in snapshot["windows"] if w["pid"] == pid and
                    w["title"] in ("Cua 3506 Synthetic A", "Cua 3506 Synthetic B")]
        if len(matching) == 2 or time.monotonic() >= deadline:
            break
        # Wait only for discovery readiness, never use this as dispatch authority.
        time.sleep(0.01)
    # Titles label the initial human-facing selection only. Every subsequent
    # operation uses exact PID+token+generation+UUID, never a title fallback.
    choices = {}
    for label in ("A", "B"):
        matches = [w for w in snapshot["windows"] if w["pid"] == pid and
                   w["title"] == "Cua 3506 Synthetic " + label]
        if len(matches) != 1:
            raise RuntimeError("initial_fixture_selection_ambiguous")
        choices[label] = matches[0]
    return owner, snapshot["generation"], choices


def case(name, action, expect=None):
    before = state()
    start = time.monotonic_ns()
    try:
        result = action()
        if expect is not None:
            raise AssertionError("expected refusal " + expect)
        assert result["confirmed"] and result["fresh_active"], result
    except ValueError as error:
        if expect != str(error):
            raise
        result = {"refusal": str(error)}
    after = state()
    for label in set(before["windows"]) & set(after["windows"]):
        for count in ("presses", "releases", "clicks", "button_releases"):
            assert before["windows"][label][count] == after["windows"][label][count]
    output.write(json.dumps({"case": name, "start_ns": start,
                             "end_ns": time.monotonic_ns(), "result": result,
                             "before": before, "after": after,
                             "input_transport_called": False}) + "\n")


owner, generation, targets = selected()
case("B_initial_activation", lambda: observe(bus, pid, targets["B"]["token"], generation))
case("A_exact_same_process_activation", lambda: observe(bus, pid, targets["A"]["token"], generation))
case("B_deliberate_exact_selection", lambda: observe(bus, pid, targets["B"]["token"], generation))

# Actual wrong active window: no setter, so observing A while B is active fails.
wrong = observe(bus, pid, targets["A"]["token"], generation, activate=False)
assert not wrong["confirmed"] and not wrong["fresh_active"]
output.write(json.dumps({"case": "wrong_active_before_confirmation", "result": wrong,
                         "state": state(), "input_transport_called": False}) + "\n")
case("lost_confirmation_timeout", lambda: observe(bus, pid, targets["A"]["token"], generation,
                                                  drop_confirmation=True),
     "confirmation_timeout_zero_input")
case("missing_exact_token", lambda: observe(bus, pid, 2**63 - 1, generation),
     "exact_target_missing_or_ambiguous")
fixture.Close("A")
case("closed_target", lambda: observe(bus, pid, targets["A"]["token"], generation),
     "exact_target_missing_or_ambiguous")
fixture.Recreate("A")
case("recreated_target_rejects_old_token", lambda: observe(bus, pid, targets["A"]["token"], generation),
     "exact_target_missing_or_ambiguous")
owner, generation, targets = selected()
case("new_selection_after_recreation", lambda: observe(bus, pid, targets["A"]["token"], generation))
subprocess.run(["qdbus6", "org.kde.KWin", "/Effects", "org.kde.kwin.Effects.unloadEffect",
                "cua_kwin_3506_proof"], check=True)
subprocess.run(["qdbus6", "org.kde.KWin", "/Effects", "org.kde.kwin.Effects.loadEffect",
                "cua_kwin_3506_proof"], check=True)
new_owner, _, snapshot = trusted_snapshot(bus)
assert new_owner == owner
assert snapshot["generation"] != generation
case("same_kwin_owner_helper_reload_stale_generation", lambda: observe(
    bus, pid, targets["A"]["token"], generation), "helper_generation_changed")
owner, generation, targets = selected()
case("fresh_resolution_after_helper_reload", lambda: observe(bus, pid, targets["A"]["token"], generation))
output.close()
print(json.dumps({"file": str(args.directory / "activation-cases.jsonl"),
                  "owner": owner, "generation": generation,
                  "fixture_pid": pid, "targets": targets,
                  "transport": "not_started"}))
