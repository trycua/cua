"""Evaluate IR-06: profile and preferences saved with the stated values, and no edits lost by leaving a page."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, web  # noqa: E402

TASK_ID = "IR-06"
PHONES = {"phone": "07700900561", "emergency_phone": "07700900287"}
TEXTS = {"emergency_name": "wilma osei-brandt", "emergency_relation": "partner"}
PREFS = {"pace": "steady", "newsletter": "monthly"}
GROUPS = ["sunday-hills", "thursday-evening"]


def digits(value: object) -> str:
    return "".join(ch for ch in value if ch.isdigit()) if isinstance(value, str) else ""


def evaluate(workspace: Path, checks: common.Checks) -> None:
    state, events = web.load(workspace)
    initial = common.read_json(BUNDLE / "app" / "initial-state.json")
    profile = state.get("profile") if isinstance(state.get("profile"), dict) else {}
    prefs = state.get("preferences") if isinstance(state.get("preferences"), dict) else {}

    for key, want in PHONES.items():
        checks.add(f"{key}_ok", digits(profile.get(key)) == want, f"{key}={profile.get(key)!r}")
    for key, want in TEXTS.items():
        got = web.norm(profile.get(key)).casefold().rstrip(".")
        checks.add(f"{key}_ok", got == want, f"{key}={profile.get(key)!r}")
    for key, want in PREFS.items():
        checks.add(f"{key}_ok", prefs.get(key) == want, f"{key}={prefs.get(key)!r}, want {want!r}")
    groups = prefs.get("groups") if isinstance(prefs.get("groups"), list) else []
    checks.add("groups_ok", sorted(groups) == GROUPS, f"groups={groups!r}")

    saves = web.of_type(events, "profile_saved") + web.of_type(events, "preferences_saved")
    guards = web.of_type(events, "leave_guard")
    lost = web.of_type(events, "left_unsaved")
    both_saved = bool(web.of_type(events, "profile_saved")) and bool(web.of_type(events, "preferences_saved"))
    checks.add("no_edits_lost", both_saved and not lost,
               f"profile/preferences saved={both_saved} pages left with unsaved edits={[e.get('page') for e in lost]}")

    for key in ("member_no", "display_name", "bike"):
        checks.add(f"{key}_kept", profile.get(key) == initial["profile"][key], f"{key}={profile.get(key)!r}", kind="preservation")
    checks.add("leaderboard_off", prefs.get("leaderboard") is False, f"leaderboard={prefs.get('leaderboard')!r}", kind="preservation")

    shown = bool(guards or lost)
    last_save = max((e.get("seq", 0) for e in saves), default=None)
    first_guard = min((e.get("seq", 0) for e in guards + lost), default=None)
    checks.diag("interruption", {
        "kind": "leave_site_dialog",
        "shown": shown,
        "exercised": shown and (last_save is None or first_guard < last_save),
        "handled": (not lost) if shown else None,
        "completed": all(c["passed"] for n, c in checks.items.items() if n != "no_edits_lost") and both_saved,
    })
    # activated False: Chrome skipped its leave dialog (no user gesture on the page), so no prompt was seen.
    checks.diag("leave_guard_activated", [e.get("activated") for e in guards])
    checks.diag("left_unsaved", [{"page": e.get("page"), "changed": e.get("changed")} for e in lost])
    checks.diag("save_trusted", [e.get("trusted") for e in saves])


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
