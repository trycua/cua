"""Verify the reset of NA-01 (in the VM: the calendar holds exactly the five fixture events)."""

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, mac  # noqa: E402

STATE = common.read_json(BUNDLE / "reset" / "app_state.json")
KEYS = ("summary", "start", "end", "location")


def app_state(workspace: Path) -> list[str]:
    return _own(workspace) + mac.calendar_strays([STATE["calendar"]], [], STATE["purge"])


def _own(workspace: Path) -> list[str]:
    data = mac.calendar_export([STATE["calendar"]])
    if data["calendars"].count(STATE["calendar"]) != 1:
        return [f"calendar count {data['calendars'].count(STATE['calendar'])}"]
    got = sorted(tuple(e[k] for k in KEYS) for e in data["events"])
    want = sorted(tuple(e[k] for k in KEYS) for e in STATE["events"])
    return [] if got == want else [f"events {got!r}"]


if __name__ == "__main__":
    common.app_verify_main(BUNDLE, "NA-01", None, app_state)
