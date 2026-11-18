"""The whole public surface of every claimed e2b release (2.42.0, 2.56.0,
2.58.0): each public module, class, function, method, field and enum value of
the real package exists in cua-e2b with the same signature, unless
`conformance/surface_waivers.py` names it with a reason.

Pure Python; runs everywhere (the goldens are recorded from the real
releases: `python -m conformance.surface` in an env with that release).
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from conformance import surface
from conformance.surface_waivers import WAIVERS, waiver_for

GOLDEN = Path(__file__).resolve().parent / "conformance" / "golden"
VERSIONS = sorted(p.name.split("-")[1] for p in GOLDEN.glob("e2b-*-surface.json"))


def _problems(version: str):
    golden = json.loads((GOLDEN / f"e2b-{version}-surface.json").read_text())
    return surface.compare(golden, surface.dump())


def test_the_claimed_releases_have_surface_goldens():
    assert VERSIONS == ["2.42.0", "2.56.0", "2.58.0"]


@pytest.mark.parametrize("version", VERSIONS)
def test_every_public_name_is_mirrored_or_waived(version):
    open_problems = [p for p in _problems(version) if waiver_for(p) is None]
    assert open_problems == [], "\n".join(open_problems[:80])


def test_no_waiver_is_stale():
    all_problems = [p for v in VERSIONS for p in _problems(v)]
    stale = [w[0] for w in WAIVERS if not any(waiver_for(p) == w for p in all_problems)]
    assert stale == []


def test_every_waiver_states_a_reason_and_a_known_status():
    for pattern, status, reason in WAIVERS:
        assert status in ("internal", "native-error"), pattern
        assert len(reason) > 20, pattern
