"""`COVERAGE.md` is generated from the recorded surfaces, the waivers and the
evidence map; this keeps all three honest."""

from __future__ import annotations

import re
from pathlib import Path

import pytest
from conformance import matrix
from conformance.coverage_evidence import EVIDENCE
from conformance.native_errors import LIMITS

HERE = Path(__file__).resolve().parent
PKG = HERE.parent


def test_coverage_md_is_current():
    committed = (PKG / "COVERAGE.md").read_text()
    assert (
        committed == matrix.render()
    ), "COVERAGE.md is stale: run `python -m conformance.matrix > ../COVERAGE.md` in tests/"


def test_no_name_is_open():
    assert [r for r in matrix.rows() if r[3] == "OPEN"] == []


def test_every_implemented_class_and_function_has_evidence():
    kinds = {r[0]: r[1] for r in matrix.rows()}
    missing = [
        p
        for p, k, _rel, st, _d in matrix.rows()
        if st == "implemented"
        and k != "member"
        and k != "constructor"
        and not matrix.evidence_for(p)
    ]
    assert missing == [], missing[:20]
    assert kinds


def _references():
    for _pattern, refs in EVIDENCE:
        for ref in (r.strip() for r in refs.split(";")):
            yield ref.split(" (")[0]


@pytest.mark.parametrize("ref", sorted(set(_references())))
def test_every_evidence_reference_exists(ref):
    if ref.startswith("conformance:"):
        assert re.search(
            rf"^async def {ref.split(':', 1)[1]}\(",
            (HERE / "conformance/suite.py").read_text(),
            re.M,
        ), ref
        return
    path, _, test = ref.partition("::")
    file = PKG / path
    assert file.exists(), ref
    if test:
        assert re.search(rf"^(async )?def {test}\(", file.read_text(), re.M), ref


@pytest.mark.parametrize("limit", LIMITS, ids=[f"{lim[0]}|{lim[1]}" for lim in LIMITS])
def test_every_documented_limit_has_a_test(limit):
    api, case, outcome, reason, test = limit
    assert outcome == "difference" or outcome.startswith("native-error: ")
    assert len(reason) > 40
    path, _, name = test.partition("::")
    file = PKG / path
    assert file.exists(), test
    if name:
        assert re.search(rf"^(async )?def {name}\(", file.read_text(), re.M), test
