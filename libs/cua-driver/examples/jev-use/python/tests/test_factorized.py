from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from factorized import (
    GATE_THRESHOLD,
    build_factorized_questions,
    choose_factorized,
    parse_factorized_decision,
    state_digest,
)
from jev_backends import JevProtocolError, read_jev_config

CANDIDATES = {
    "type-verification-value": "Replace the verification field.",
    "reobserve": "Obtain a fresh observation.",
    "abstain": "Stop without acting.",
}


def answers(choice="type-verification-value", confidence=0.9, goal=0.1, reobserve=0.05):
    rest = (1.0 - confidence) / 2
    return {
        "selection": {
            "type": "choice",
            "choice": choice,
            "confidence": confidence,
            "probabilities": {
                cid: (confidence if cid == choice else rest) for cid in CANDIDATES
            },
        },
        "goal_achieved": {"type": "noul", "noul": goal},
        "needs_reobserve": {"type": "noul", "noul": reobserve},
    }


def stub_transport(payload_answers):
    def transport(url, payload, headers, timeout):
        return {"model": "stub-jev", "answers": payload_answers}

    return transport


class BuildQuestionsTest(unittest.TestCase):
    def test_three_questions_with_types(self) -> None:
        questions = build_factorized_questions(CANDIDATES, goal="Submit the form.")
        self.assertEqual(
            set(questions), {"selection", "goal_achieved", "needs_reobserve"}
        )
        self.assertEqual(questions["selection"]["type"], "choice")
        self.assertEqual(questions["goal_achieved"]["type"], "noul")
        self.assertEqual(questions["needs_reobserve"]["type"], "noul")
        self.assertEqual(questions["selection"]["criteria"], CANDIDATES)

    def test_goal_required(self) -> None:
        with self.assertRaises(ValueError):
            build_factorized_questions(CANDIDATES, goal="  ")


class ParseDecisionTest(unittest.TestCase):
    def test_happy_path_keeps_selection(self) -> None:
        decision = parse_factorized_decision(answers(), CANDIDATES, backend="local")
        assert decision is not None
        self.assertEqual(decision.selected_id, "type-verification-value")
        self.assertEqual(decision.backend, "local")
        self.assertAlmostEqual(decision.goal_achieved, 0.1)

    def test_goal_achieved_gate_fires_abstain(self) -> None:
        decision = parse_factorized_decision(
            answers(goal=GATE_THRESHOLD + 0.1), CANDIDATES
        )
        assert decision is not None
        self.assertEqual(decision.selected_id, "abstain")

    def test_needs_reobserve_gate_fires_reobserve(self) -> None:
        decision = parse_factorized_decision(
            answers(reobserve=GATE_THRESHOLD + 0.1), CANDIDATES
        )
        assert decision is not None
        self.assertEqual(decision.selected_id, "reobserve")

    def test_gates_need_reserved_ids(self) -> None:
        no_reserved = {"a": "Do A.", "b": "Do B."}
        raw = answers(goal=0.99, reobserve=0.99)
        raw["selection"]["choice"] = "a"
        raw["selection"]["probabilities"] = {"a": 0.9, "b": 0.1}
        decision = parse_factorized_decision(raw, no_reserved)
        assert decision is not None
        self.assertEqual(decision.selected_id, "a")

    def test_low_confidence_fails_open(self) -> None:
        self.assertIsNone(parse_factorized_decision(answers(confidence=0.1), CANDIDATES))

    def test_malformed_answers_fail_open(self) -> None:
        raw = answers()
        raw["selection"]["probabilities"]["abstain"] = 0.9  # bad mass
        self.assertIsNone(parse_factorized_decision(raw, CANDIDATES))
        raw = answers()
        del raw["needs_reobserve"]
        self.assertIsNone(parse_factorized_decision(raw, CANDIDATES))
        self.assertIsNone(parse_factorized_decision("nope", CANDIDATES))
        self.assertIsNone(parse_factorized_decision(answers(), {"a": ""}))


class StateDigestTest(unittest.TestCase):
    def test_deterministic_and_secret_free(self) -> None:
        first = state_digest("Enter the token hunter2", ["a", "b"], "c1")
        second = state_digest("Enter the token hunter2", ["b", "a"], "c1")
        self.assertEqual(first, second)
        self.assertTrue(first.startswith("sha256:"))
        self.assertNotIn("hunter2", first)


class ChooseFactorizedTest(unittest.TestCase):
    def test_mock_backend_packet(self) -> None:
        config = read_jev_config({})
        packet = choose_factorized(
            config, goal="g", observation={}, candidates=CANDIDATES, capture_id="c1"
        )
        assert packet is not None
        body = packet.to_dict()
        self.assertEqual(body["selected_id"], "type-verification-value")
        self.assertEqual(body["backend"], "mock")
        self.assertTrue(body["state_digest"].startswith("sha256:"))
        self.assertNotIn("screenshot", str(body).lower())

    def test_http_backend_packet(self) -> None:
        config = read_jev_config({"JEV_BACKEND": "local"})
        packet = choose_factorized(
            config,
            goal="g",
            observation={"page": "fixture"},
            candidates=CANDIDATES,
            capture_id="c1",
            transport=stub_transport(answers()),
        )
        assert packet is not None
        self.assertEqual(packet.decision.selected_id, "type-verification-value")
        self.assertEqual(packet.decision.backend, "local")
        self.assertGreaterEqual(packet.latency_ms, 0.0)

    def test_transport_failure_fails_open(self) -> None:
        def transport(url, payload, headers, timeout):
            raise JevProtocolError("boom")

        config = read_jev_config({"JEV_BACKEND": "local"})
        self.assertIsNone(
            choose_factorized(
                config, goal="g", observation={}, candidates=CANDIDATES,
                transport=transport,
            )
        )

    def test_bad_criteria_fails_open(self) -> None:
        config = read_jev_config({})
        self.assertIsNone(
            choose_factorized(config, goal="g", observation={}, candidates={})
        )


if __name__ == "__main__":
    unittest.main()
