"""Content-free receipts for passive actionable-domain precedence.

Extends trycua/cua#3963 still-open **passive native observations** and
kvnloo/cua#8 / trycua/cua#3904 contract vectors.

RFC #3904 is review-only (no Driver product). This leaf does **not**
implement get_window_state observations. It attributes the conservative
evaluation rule discovered on current main:

  Actionable-domain precedence — if the selector matches any actionable
  row, evaluate exactly that actionable domain and ignore passive rows
  for that predicate. Only when the actionable domain has zero selector
  matches may passive observations participate.

Passive rows remain readable for verification but never mint action
authority. Receipts are content-free.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass
from typing import Any, Literal

Status = Literal["satisfied", "unsatisfied", "unknown"]
Reason = Literal[
    "allowed_actionable",
    "allowed_passive",
    "multi_match",
    "untrusted_source",
    "unsupported_predicate",
    "incomplete_domains",
    "no_match",
    "passive_not_actionable",
]
Domain = Literal["actionable", "passive", "none"]


@dataclass(frozen=True)
class DomainRow:
    """One selector-matched row in either domain."""

    identity: str
    domain: Literal["actionable", "passive"]
    value: str | None = None
    enabled: bool | None = None
    selected: bool | None = None
    trusted: bool = True
    supports_value: bool = True


@dataclass(frozen=True)
class PassiveEvidence:
    """Content-free receipt for one predicate evaluation attempt."""

    status: Status
    reason: Reason
    domain_used: Domain
    actionable_matches: int
    passive_matches: int
    action_authority_minted: bool
    selected_identity: str | None

    def as_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True)
class ActionAuthorityEvidence:
    """Content-free receipt for an action-target request."""

    allowed: bool
    reason: Reason
    identity: str | None
    action_authority_minted: bool

    def as_dict(self) -> dict[str, Any]:
        return asdict(self)


def _property_holds(row: DomainRow, *, expect_value: str | None, expect_enabled: bool | None) -> Status | None:
    """Return satisfied/unsatisfied when the property is evaluable; None if unsupported."""
    if expect_value is not None:
        if not row.supports_value or row.value is None:
            return None
        return "satisfied" if row.value == expect_value else "unsatisfied"
    if expect_enabled is not None:
        if row.enabled is None:
            return None
        return "satisfied" if row.enabled is expect_enabled else "unsatisfied"
    # exists:true style — selector match alone satisfies when domain admits it.
    return "satisfied"


def evaluate_passive_precedence(
    actionable: list[DomainRow],
    passive: list[DomainRow],
    *,
    expect_value: str | None = None,
    expect_enabled: bool | None = None,
    exists: bool | None = None,
    actionable_complete: bool = True,
    passive_complete: bool = True,
) -> PassiveEvidence:
    """Evaluate one property/exists predicate under actionable-domain precedence.

    ``actionable`` / ``passive`` are already selector-filtered match lists.
    """
    actionable_matches = len(actionable)
    passive_matches = len(passive)

    def _receipt(
        status: Status,
        reason: Reason,
        *,
        domain_used: Domain,
        selected_identity: str | None = None,
    ) -> PassiveEvidence:
        return PassiveEvidence(
            status=status,
            reason=reason,
            domain_used=domain_used,
            actionable_matches=actionable_matches,
            passive_matches=passive_matches,
            action_authority_minted=False,
            selected_identity=selected_identity,
        )

    # --- Actionable domain wins whenever it has any selector match ---
    if actionable_matches >= 1:
        if actionable_matches > 1 and (expect_value is not None or expect_enabled is not None):
            return _receipt("unknown", "multi_match", domain_used="actionable")
        row = actionable[0]
        if not row.trusted:
            return _receipt("unknown", "untrusted_source", domain_used="actionable")
        if exists is True:
            return _receipt(
                "satisfied",
                "allowed_actionable",
                domain_used="actionable",
                selected_identity=row.identity,
            )
        hold = _property_holds(row, expect_value=expect_value, expect_enabled=expect_enabled)
        if hold is None:
            return _receipt("unknown", "unsupported_predicate", domain_used="actionable")
        return _receipt(
            hold,
            "allowed_actionable",
            domain_used="actionable",
            selected_identity=row.identity if hold == "satisfied" else None,
        )

    # --- Passive participates only when actionable matched zero ---
    if passive_matches == 0:
        if actionable_complete and passive_complete:
            return _receipt("unsatisfied", "no_match", domain_used="none")
        return _receipt("unknown", "incomplete_domains", domain_used="none")

    if passive_matches > 1 and (expect_value is not None or expect_enabled is not None):
        return _receipt("unknown", "multi_match", domain_used="passive")

    row = passive[0]
    if not row.trusted:
        return _receipt("unknown", "untrusted_source", domain_used="passive")

    if exists is True:
        return _receipt(
            "satisfied",
            "allowed_passive",
            domain_used="passive",
            selected_identity=row.identity,
        )

    hold = _property_holds(row, expect_value=expect_value, expect_enabled=expect_enabled)
    if hold is None:
        return _receipt("unknown", "unsupported_predicate", domain_used="passive")
    return _receipt(
        hold,
        "allowed_passive",
        domain_used="passive",
        selected_identity=row.identity if hold == "satisfied" else None,
    )


def explain_action_authority(row: DomainRow) -> ActionAuthorityEvidence:
    """Passive rows are never action targets; actionable rows may be."""
    if row.domain == "passive":
        return ActionAuthorityEvidence(
            allowed=False,
            reason="passive_not_actionable",
            identity=None,
            action_authority_minted=False,
        )
    return ActionAuthorityEvidence(
        allowed=True,
        reason="allowed_actionable",
        identity=row.identity,
        action_authority_minted=True,
    )