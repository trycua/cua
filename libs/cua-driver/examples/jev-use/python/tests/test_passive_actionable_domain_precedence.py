"""Actionable-domain precedence + no action authority for passive rows.

Invariant (kvnloo/cua#8 / trycua/cua#3904 / #3963 passive native observations):
  If any actionable row matches the selector, evaluate that domain only —
  passive rows cannot introduce multi_match or mint action authority.
  Passive participates only when actionable matches are zero.
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

BASE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BASE / "python"))

from passive_actionable_domain import (
    DomainRow,
    evaluate_passive_precedence,
    explain_action_authority,
)


def actionable(identity: str = "btn-eq", *, value: str | None = None, **kw: object) -> DomainRow:
    return DomainRow(identity=identity, domain="actionable", value=value, **kw)  # type: ignore[arg-type]


def passive(identity: str = "calc-result", *, value: str | None = "42", **kw: object) -> DomainRow:
    return DomainRow(identity=identity, domain="passive", value=value, **kw)  # type: ignore[arg-type]


class PassiveActionableDomainPrecedenceTest(unittest.TestCase):
    def test_actionable_unique_ignores_passive_siblings(self) -> None:
        evidence = evaluate_passive_precedence(
            [actionable(value="=")],
            [passive(value="42"), passive("calc-expr", value="6x7")],
            expect_value="=",
        )
        self.assertEqual(evidence.status, "satisfied")
        self.assertEqual(evidence.reason, "allowed_actionable")
        self.assertEqual(evidence.domain_used, "actionable")
        self.assertEqual(evidence.actionable_matches, 1)
        self.assertEqual(evidence.passive_matches, 2)
        self.assertFalse(evidence.action_authority_minted)
        self.assertEqual(evidence.selected_identity, "btn-eq")

    def test_actionable_unique_unchanged_when_passive_absent(self) -> None:
        evidence = evaluate_passive_precedence(
            [actionable(value="=")],
            [],
            expect_value="=",
        )
        self.assertEqual(evidence.status, "satisfied")
        self.assertEqual(evidence.reason, "allowed_actionable")
        self.assertEqual(evidence.domain_used, "actionable")
        self.assertEqual(evidence.passive_matches, 0)

    def test_passive_only_unique_property_satisfies(self) -> None:
        evidence = evaluate_passive_precedence(
            [],
            [passive(value="42")],
            expect_value="42",
        )
        self.assertEqual(evidence.status, "satisfied")
        self.assertEqual(evidence.reason, "allowed_passive")
        self.assertEqual(evidence.domain_used, "passive")
        self.assertEqual(evidence.actionable_matches, 0)
        self.assertFalse(evidence.action_authority_minted)
        self.assertEqual(evidence.selected_identity, "calc-result")

    def test_passive_multi_match_property_is_unknown(self) -> None:
        evidence = evaluate_passive_precedence(
            [],
            [passive("a", value="42"), passive("b", value="42")],
            expect_value="42",
        )
        self.assertEqual(evidence.status, "unknown")
        self.assertEqual(evidence.reason, "multi_match")
        self.assertEqual(evidence.domain_used, "passive")
        self.assertIsNone(evidence.selected_identity)

    def test_passive_exists_when_actionable_empty(self) -> None:
        evidence = evaluate_passive_precedence(
            [],
            [passive(value="42")],
            exists=True,
        )
        self.assertEqual(evidence.status, "satisfied")
        self.assertEqual(evidence.reason, "allowed_passive")
        self.assertEqual(evidence.domain_used, "passive")

    def test_untrusted_passive_only_is_unknown(self) -> None:
        evidence = evaluate_passive_precedence(
            [],
            [passive(value="42", trusted=False)],
            expect_value="42",
        )
        self.assertEqual(evidence.status, "unknown")
        self.assertEqual(evidence.reason, "untrusted_source")
        self.assertEqual(evidence.domain_used, "passive")

    def test_unsupported_passive_property_is_unknown(self) -> None:
        evidence = evaluate_passive_precedence(
            [],
            [passive(value=None, supports_value=False)],
            expect_value="42",
        )
        self.assertEqual(evidence.status, "unknown")
        self.assertEqual(evidence.reason, "unsupported_predicate")

    def test_no_match_complete_domains_unsatisfied(self) -> None:
        evidence = evaluate_passive_precedence(
            [],
            [],
            expect_value="42",
            actionable_complete=True,
            passive_complete=True,
        )
        self.assertEqual(evidence.status, "unsatisfied")
        self.assertEqual(evidence.reason, "no_match")
        self.assertEqual(evidence.domain_used, "none")

    def test_no_match_incomplete_is_unknown(self) -> None:
        evidence = evaluate_passive_precedence(
            [],
            [],
            expect_value="42",
            actionable_complete=True,
            passive_complete=False,
        )
        self.assertEqual(evidence.status, "unknown")
        self.assertEqual(evidence.reason, "incomplete_domains")

    def test_passive_never_mints_action_authority(self) -> None:
        refused = explain_action_authority(passive())
        allowed = explain_action_authority(actionable())
        self.assertFalse(refused.allowed)
        self.assertEqual(refused.reason, "passive_not_actionable")
        self.assertFalse(refused.action_authority_minted)
        self.assertIsNone(refused.identity)
        self.assertTrue(allowed.allowed)
        self.assertTrue(allowed.action_authority_minted)
        self.assertEqual(allowed.identity, "btn-eq")

    def test_receipt_dict_is_content_free(self) -> None:
        evidence = evaluate_passive_precedence(
            [actionable(value="=")],
            [passive(value="secret-token")],
            expect_value="=",
        )
        payload = evidence.as_dict()
        text = str(payload).lower()
        for forbidden in ("secret-token", "password", "authorization", "image_base64"):
            self.assertNotIn(forbidden, text)
        self.assertEqual(
            set(payload),
            {
                "status",
                "reason",
                "domain_used",
                "actionable_matches",
                "passive_matches",
                "action_authority_minted",
                "selected_identity",
            },
        )
        self.assertFalse(payload["action_authority_minted"])


if __name__ == "__main__":
    unittest.main()