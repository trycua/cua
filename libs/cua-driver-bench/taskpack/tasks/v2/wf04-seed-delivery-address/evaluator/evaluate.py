"""Evaluate WF-04: delivery and separate billing addresses saved as stated; pallet freight; notes kept."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, web  # noqa: E402

TASK_ID = "WF-04"
SHIPPING = {"name": "Joris Elzinga", "line1": "Hoefslagpad 23", "line2": "Greenhouse 4", "town": "Leeuwarden",
            "region": "Friesland", "postcode": "8912KL", "country": "NL"}
BILLING = {"name": "Joris Elzinga", "line1": "Am Lindenhof 6", "line2": "", "town": "Oldenburg",
           "region": "Lower Saxony", "postcode": "26121", "country": "DE"}


def same(key: str, got: object, want: str) -> bool:
    if not isinstance(got, str):
        return False
    if key == "postcode":
        return got.replace(" ", "").upper() == want
    if key in ("country", "region"):
        return got == want
    return web.norm(got).lower() == want.lower()


def evaluate(workspace: Path, checks: common.Checks) -> None:
    state, events = web.load(workspace)
    initial = common.read_json(BUNDLE / "app" / "initial-state.json")
    d = state.get("delivery") if isinstance(state.get("delivery"), dict) else {}
    ship = d.get("shipping") if isinstance(d.get("shipping"), dict) else {}
    bill = d.get("billing") if isinstance(d.get("billing"), dict) else {}
    checks.add("saved", d.get("status") == "saved", f"status={d.get('status')!r}")
    for key, want in SHIPPING.items():
        checks.add(f"shipping_{key}_ok", same(key, ship.get(key), want), f"shipping.{key}={ship.get(key)!r}, want {want!r}")
    checks.add("billing_separate", d.get("billing_same") is False and bool(bill), f"billing_same={d.get('billing_same')!r}")
    for key, want in BILLING.items():
        checks.add(f"billing_{key}_ok", bool(bill) and same(key, bill.get(key), want), f"billing.{key}={bill.get(key)!r}, want {want!r}")
    checks.add("option_pallet", d.get("option") == "pallet", f"option={d.get('option')!r}")
    checks.add("notes_kept", d.get("notes") == initial["delivery"]["notes"], f"notes={d.get('notes')!r}", kind="preservation")
    checks.add("order_kept", state.get("order") == initial["order"], f"order={state.get('order')!r}", kind="preservation")
    checks.diag("saves", [e.get("trusted") for e in web.of_type(events, "delivery_saved")])


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
