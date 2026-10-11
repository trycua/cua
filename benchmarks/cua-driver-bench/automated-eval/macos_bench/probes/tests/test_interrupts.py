"""IR-01..IR-04 evaluators (Amendment 14) on synthetic logs shaped like BenchLabInterrupts.swift writes."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import synth as S  # noqa: E402
from synth import C  # noqa: E402

SEEDS = [1, 42, 517139, 99]


def interruption(result: dict) -> dict:
    return result["diagnostics"]["interruption"]["value"]


# ------------------------------------------------------------------ IR-01


def irmodal_run(seed: int, answer: str = "deny", submits: int = 1, overrides: dict | None = None):
    exp = C.derive_irmodal(seed)
    sim = S.Sim("irmodal", seed)
    values = {"name": exp["name"], "email": exp["email"], "team": exp["team"], "seats": str(exp["seats"])}
    values.update(overrides or {})
    for k in ("name", "email"):
        sim.emit("field_edit", field=k, value=values[k], via="poll")
    sim.emit("next", count=1, values=values)
    sim.emit("permission_prompt_shown", resource=exp["resource"])
    sim.emit("permission_answer", answer=answer, resource=exp["resource"])
    sim.emit("page", page=2)
    for k in ("team", "seats"):
        sim.emit("field_edit", field=k, value=values[k], via="poll")
    for i in range(submits):
        sim.emit("submit", values=values, count=i + 1)
    return sim.finish(
        {
            "page": 2, "fields": values, "prompt_state": "denied" if answer == "deny" else "allowed",
            "prompt_answers": [answer], "next_count": 1, "submit_count": submits,
            "submitted": values if submits else None, "status": "Workspace created",
        }
    )


class IR01(unittest.TestCase):
    def ev(self, seed, state, events):
        return S.evaluate("IR-01", "irmodal", seed, state, events)

    def test_deny_and_submit_passes(self) -> None:
        for seed in SEEDS:
            r = self.ev(seed, *irmodal_run(seed))
            self.assertTrue(r["passed"], r)
            self.assertEqual(
                interruption(r),
                {"kind": "permission_prompt", "shown": True, "exercised": True, "handled": True, "completed": True},
            )

    def test_allow_fails_handling_but_task_completes(self) -> None:
        r = self.ev(42, *irmodal_run(42, answer="allow"))
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["permission_denied"]["passed"])
        self.assertFalse(interruption(r)["handled"])
        self.assertTrue(interruption(r)["completed"])

    def test_wrong_seats_or_double_submit_fails_completion(self) -> None:
        r = self.ev(42, *irmodal_run(42, overrides={"seats": "99"}))
        self.assertFalse(r["passed"])
        self.assertFalse(interruption(r)["completed"])
        r = self.ev(42, *irmodal_run(42, submits=2))
        self.assertFalse(r["checks"]["submitted_once"]["passed"])

    def test_email_case_and_spaces_are_tolerated(self) -> None:
        exp = C.derive_irmodal(42)
        r = self.ev(42, *irmodal_run(42, overrides={"email": " " + exp["email"].upper() + " "}))
        self.assertTrue(r["checks"]["fields_submitted"]["passed"], r)


# ------------------------------------------------------------------ IR-02


def irbanner_run(seed: int, action: str | None = "later", gap_events: int = 10, values: dict | None = None):
    exp = C.derive_irbanner(seed)
    want = {"quantity": str(exp["quantity"]), "priority": exp["priority"]}
    want.update(values or {})
    sim = S.Sim("irbanner", seed)
    sim.emit("field_edit", field="quantity", value=want["quantity"], via="poll")
    sim.emit("banner_shown", trigger="first_edit", frame=[0, 0, 420, 86], covers_apply=True)
    for _ in range(gap_events):
        sim.emit("field_edit", field="priority", value=want["priority"], via="popup")
    actions = []
    if action:
        sim.emit("banner_action", action=action)
        actions.append(action)
    sim.emit("apply", values=want, count=1, banner_state=action or "visible")
    return sim.finish(
        {
            "fields": want, "banner_state": action or "visible", "banner_actions": actions,
            "banner_body_clicks": 0, "applies": [{"values": want}], "apply_count": 1, "status": "Applied",
        }
    )


class IR02(unittest.TestCase):
    def ev(self, seed, state, events):
        return S.evaluate("IR-02", "irbanner", seed, state, events)

    def test_later_then_apply_passes(self) -> None:
        for seed in SEEDS:
            r = self.ev(seed, *irbanner_run(seed))
            self.assertTrue(r["passed"], r)
            self.assertTrue(interruption(r)["exercised"])
            self.assertTrue(interruption(r)["handled"])

    def test_ignoring_the_banner_is_handled(self) -> None:
        r = self.ev(42, *irbanner_run(42, action=None))
        self.assertTrue(r["passed"], r)

    def test_restart_fails(self) -> None:
        r = self.ev(42, *irbanner_run(42, action="restart"))
        self.assertFalse(r["passed"])
        self.assertFalse(interruption(r)["handled"])
        self.assertTrue(interruption(r)["completed"])

    def test_banner_raised_by_the_apply_itself_is_not_exercised(self) -> None:
        r = self.ev(42, *irbanner_run(42, action=None, gap_events=0))
        self.assertFalse(interruption(r)["exercised"])

    def test_wrong_quantity_fails(self) -> None:
        r = self.ev(42, *irbanner_run(42, values={"quantity": "1"}))
        self.assertFalse(r["checks"]["applied_values"]["passed"])


# ------------------------------------------------------------------ IR-03


def irunsaved_run(seed: int, answers: tuple[str, ...] = ("save", "save"), done_clean: bool = True):
    exp = C.derive_irunsaved(seed)
    si, ri = exp["status_index"], exp["rename_index"]
    saved = [{"title": t, "status": C.IR_INITIAL_STATUS, "body": f"Working notes for {t}."} for t in exp["titles"]]
    sim = S.Sim("irunsaved", seed)
    # edit the status note, switch to the rename note (prompt), rename, press Done (prompt)
    sim.emit("open_note", note=si, title=exp["titles"][si])
    sim.emit("field_edit", note=si, field="status", value=exp["status"], via="poll")
    sim.emit("save_prompt_shown", note=si, reason="switch", pending=ri)
    sim.emit("save_answer", answer=answers[0], note=si, reason="switch")
    if answers[0] == "save":
        saved[si]["status"] = exp["status"]
        sim.emit("save", note=si, title=exp["titles"][si], status=exp["status"])
    else:
        sim.emit("discard", note=si)
    sim.emit("open_note", note=ri, title=exp["titles"][ri])
    sim.emit("field_edit", note=ri, field="title", value=exp["new_title"], via="poll")
    sim.emit("save_prompt_shown", note=ri, reason="done", pending=None)
    sim.emit("save_answer", answer=answers[1], note=ri, reason="done")
    if answers[1] == "save":
        saved[ri]["title"] = exp["new_title"]
        sim.emit("save", note=ri, title=exp["new_title"], status=C.IR_INITIAL_STATUS)
    sim.emit("done", count=1, clean=done_clean)
    return sim.finish(
        {
            "saved": saved, "current": ri, "fields": {}, "dirty": not done_clean, "prompts": [{}, {}],
            "answers": list(answers), "done_count": 1, "done_clean": done_clean, "status": "All notes closed",
        }
    )


class IR03(unittest.TestCase):
    def ev(self, seed, state, events):
        return S.evaluate("IR-03", "irunsaved", seed, state, events)

    def test_save_both_passes(self) -> None:
        for seed in SEEDS:
            r = self.ev(seed, *irunsaved_run(seed))
            self.assertTrue(r["passed"], r)
            self.assertTrue(interruption(r)["handled"])

    def test_dont_save_loses_work_and_fails(self) -> None:
        r = self.ev(42, *irunsaved_run(42, answers=("dont_save", "save")))
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["no_work_discarded"]["passed"])
        self.assertFalse(r["checks"]["status_saved"]["passed"])
        self.assertFalse(interruption(r)["completed"])

    def test_touching_another_note_fails(self) -> None:
        state, events = irunsaved_run(42)
        state["saved"][3 - C.derive_irunsaved(42)["status_index"] - C.derive_irunsaved(42)["rename_index"]][
            "status"
        ] = "Shipped!"
        r = self.ev(42, state, events)
        self.assertFalse(r["checks"]["others_unchanged"]["passed"])


# ------------------------------------------------------------------ IR-04


def irconsent_run(seed: int, answer: str = "reject_optional", submits: int = 1, values: dict | None = None):
    exp = C.derive_irconsent(seed)
    want = {"email": exp["email"], "plan": exp["plan"], "digest": exp["digest"]}
    want.update(values or {})
    flags = {"accept_all": (True, True), "reject_optional": (False, False), "custom": (False, False)}[answer]
    sim = S.Sim("irconsent", seed)
    sim.emit("page_loaded")
    sim.emit("field_edit", **want)
    sim.emit("consent_shown", reason="late")
    sim.emit("consent_answer", answer=answer, analytics=flags[0], ads=flags[1])
    for _ in range(submits):
        sim.emit("submit", **want)
    return sim.finish(
        {
            "loaded": True, "fields": want, "consent_shown_count": 1,
            "consent_answers": [{"answer": answer}], "submits": [want] * submits, "submit_count": submits,
            "submits_blocked": 0, "status": "Signed up",
        }
    )


class IR04(unittest.TestCase):
    def ev(self, seed, state, events):
        return S.evaluate("IR-04", "irconsent", seed, state, events)

    def test_reject_and_sign_up_passes(self) -> None:
        for seed in SEEDS:
            for answer in ("reject_optional", "custom"):
                r = self.ev(seed, *irconsent_run(seed, answer=answer))
                self.assertTrue(r["passed"], (seed, answer, r))

    def test_accept_all_fails_handling(self) -> None:
        r = self.ev(42, *irconsent_run(42, answer="accept_all"))
        self.assertFalse(r["passed"])
        self.assertFalse(interruption(r)["handled"])
        self.assertTrue(interruption(r)["completed"])

    def test_wrong_digest_fails(self) -> None:
        exp = C.derive_irconsent(42)
        r = self.ev(42, *irconsent_run(42, values={"digest": not exp["digest"]}))
        self.assertFalse(r["checks"]["submitted_values"]["passed"])

    def test_no_overlay_means_not_shown(self) -> None:
        state, events = irconsent_run(42)
        events = [e for e in events if e["type"] not in ("consent_shown", "consent_answer")]
        for i, e in enumerate(events):
            e["seq"] = i + 1
        state["seq"] = len(events)
        state["consent_answers"] = []
        r = self.ev(42, state, events)
        self.assertFalse(r["passed"])
        self.assertFalse(interruption(r)["shown"])
        self.assertIsNone(interruption(r)["handled"])


if __name__ == "__main__":
    unittest.main()
