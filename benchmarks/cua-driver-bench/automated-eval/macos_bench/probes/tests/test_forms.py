from __future__ import annotations

import unittest

import synth
from synth import C

PROBE, MODE = "PROBE-FORMS", "forms"
SEEDS = [1, 7, 42, 4242, 999_999]


def run(seed, state, events, bad=0):
    return synth.evaluate(PROBE, MODE, seed, state, events, bad)


class FormsEvaluatorTests(unittest.TestCase):
    def assertFailsOnly(self, result, *names):
        failed = sorted(n for n, c in result["checks"].items() if not c["passed"])
        self.assertEqual(failed, sorted(names), result["checks"])
        self.assertFalse(result["passed"])

    def test_genuine_run_passes_for_many_seeds(self):
        for seed in SEEDS:
            state, events = synth.forms_run(seed)
            result = run(seed, state, events)
            self.assertTrue(result["passed"], (seed, result["checks"]))
            self.assertEqual(result["score"], 1.0)
            self.assertAlmostEqual(sum(c["weight"] for c in result["checks"].values()), 1.0)

    def test_amount_is_compared_numerically(self):
        seed = 7
        exp = C.derive_forms(seed)
        dollars, cents = exp["invoice_amount"].split(".")
        for typed in (
            f"{int(dollars):,}.{cents}",
            f"${dollars}.{cents}",
            f" {dollars}.{cents} ",
            exp["invoice_amount"] + "0",
        ):
            state, events = synth.forms_run(seed, {"invoice_amount": typed})
            self.assertTrue(run(seed, state, events)["passed"], typed)
        for typed in (
            f"{int(dollars) + 1}.{cents}",
            f"{dollars}.{(int(cents) + 1) % 100:02d}",
            "abc",
            "",
        ):
            state, events = synth.forms_run(seed, {"invoice_amount": typed})
            self.assertFailsOnly(run(seed, state, events), "invoice_amount")

    def test_each_wrong_field_fails_only_that_check(self):
        seed = 42
        exp = C.derive_forms(seed)
        other_cat = (exp["category_index"] % 7) + 1
        other_prio = next(p for p in C.PRIORITIES if p != exp["priority"])
        cases = {
            "customer_name": ({"customer_name": exp["customer_name"] + "x"}, "customer_name"),
            "category": (
                {"category_index": other_cat, "category_title": C.CATEGORIES[other_cat]},
                "category",
            ),
            "priority": ({"priority": other_prio}, "priority"),
            "notify": ({"notify": not exp["notify"]}, "notify"),
            "quantity": (
                {"quantity": exp["quantity"] + 1, "quantity_text": str(exp["quantity"] + 1)},
                "quantity",
            ),
            "notes": ({"notes": exp["notes"].lower()}, "notes"),
        }
        for label, (override, check) in cases.items():
            state, events = synth.forms_run(seed, override)
            self.assertFailsOnly(run(seed, state, events), check)

    def test_surrounding_whitespace_in_text_fields_is_tolerated(self):
        seed = 1
        exp = C.derive_forms(seed)
        state, events = synth.forms_run(
            seed, {"customer_name": f"  {exp['customer_name']} ", "notes": exp["notes"] + "\n"}
        )
        self.assertTrue(run(seed, state, events)["passed"])

    def test_missing_priority_and_null_quantity_fail(self):
        seed = 1
        state, events = synth.forms_run(seed, {"priority": None})
        self.assertIn(
            "priority",
            [n for n, c in run(seed, state, events)["checks"].items() if not c["passed"]],
        )
        state, events = synth.forms_run(seed, {"quantity": None, "quantity_text": "x"})
        self.assertIn(
            "quantity",
            [n for n, c in run(seed, state, events)["checks"].items() if not c["passed"]],
        )

    def test_not_submitted_fails_everything_except_integrity(self):
        seed = 7
        state, events = synth.forms_run(seed)
        events = [e for e in events if e["type"] != "submit"]
        state["submitted"], state["submit_count"], state["submission"], state["status"] = (
            False,
            0,
            None,
            "",
        )
        state["seq"] = events[-1]["seq"]
        result = run(seed, state, events)
        self.assertFalse(result["passed"])
        self.assertTrue(result["checks"]["integrity"]["passed"])
        self.assertEqual(sum(c["passed"] for c in result["checks"].values()), 1)
        self.assertEqual(result["score"], 0.05)

    def test_submitted_twice_fails_submitted_once(self):
        seed = 7
        state, events = synth.forms_run(seed, submits=2)
        self.assertFailsOnly(run(seed, state, events), "submitted_once")

    def test_state_edited_without_ui_events_fails(self):
        seed = 7
        state, events = synth.forms_run(seed)
        events = events[:1]  # only app_start survives
        state["seq"] = 1
        result = run(seed, state, events)
        self.assertFalse(result["passed"])
        self.assertFalse(result["checks"]["ui_events"]["passed"])
        self.assertFalse(result["checks"]["submitted_once"]["passed"])

    def test_state_snapshot_edited_after_the_fact_fails_ui_events(self):
        seed = 7
        exp = C.derive_forms(seed)
        state, events = synth.forms_run(seed, {"customer_name": "Someone Else"})
        # Forge the snapshot to the right answer while the event log still shows the wrong one.
        state["submission"]["fields"]["customer_name"] = exp["customer_name"]
        state["fields"] = state["submission"]["fields"]
        result = run(seed, state, events)
        self.assertFalse(result["checks"]["ui_events"]["passed"])
        self.assertFalse(result["passed"])

    def test_value_without_a_matching_edit_event_fails_ui_events(self):
        seed = 7
        state, events = synth.forms_run(seed)
        events = [
            e
            for e in events
            if not (e["type"] == "field_edit" and e["details"]["field"] == "notes")
        ]
        for i, e in enumerate(events, 1):
            e["seq"] = i
        state["seq"] = len(events)
        result = run(seed, state, events)
        self.assertFailsOnly(result, "ui_events")

    def test_wrong_seed_or_mode_fails_integrity(self):
        state, events = synth.forms_run(7)
        self.assertFalse(run(8, state, events)["checks"]["integrity"]["passed"])
        state2, events2 = synth.clone(state, events)
        state2["mode"] = "table"
        self.assertFalse(run(7, state2, events2)["checks"]["integrity"]["passed"])

    def test_event_log_tampering_fails_integrity(self):
        seed = 7
        state, events = synth.forms_run(seed)
        gap = [e for i, e in enumerate(events) if i != 3]
        self.assertFalse(run(seed, state, gap)["checks"]["integrity"]["passed"])
        self.assertFalse(run(seed, state, events, bad=1)["checks"]["integrity"]["passed"])
        self.assertFalse(run(seed, state, events[1:])["checks"]["integrity"]["passed"])
        self.assertFalse(run(seed, state, [])["passed"])

    def test_garbage_state_fails_closed(self):
        seed = 7
        _, events = synth.forms_run(seed)
        for state in ({}, {"mode": "forms", "seed": seed, "seq": len(events), "submission": "x"}):
            result = run(seed, state, events)
            self.assertFalse(result["passed"])
            self.assertLess(result["score"], 1.0)


if __name__ == "__main__":
    unittest.main()
