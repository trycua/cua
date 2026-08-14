"""The Windows installers must not claim autostart is registered when it isn't.

Both installers used to gate their closing `Auto-start: '...' is registered at
RunLevel=Highest.` line on the `-AutoStart` *request* rather than on the
registration *outcome*, so a declined UAC prompt produced an install transcript
that confirmed a task which was never created (trycua/cua#3179).
"""

from __future__ import annotations

from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1]

# (installer, closing summary line, task name)
INSTALLERS = [
    pytest.param(
        SCRIPTS / "install.ps1",
        "Auto-start: 'cua-driver-serve' is registered at RunLevel=Highest.",
        id="install.ps1",
    ),
    pytest.param(
        SCRIPTS / "install-local.ps1",
        "Auto-start: 'cua-driver-local-serve' is registered at RunLevel=Highest.",
        id="install-local.ps1",
    ),
]

# A branch opener in these scripts, at the granularity this test cares about.
BRANCH_PREFIXES = ("if (", "} elseif (", "elseif (", "} else", "else {")


def _guard_for(lines: list[str], index: int) -> str:
    """The nearest branch opener above `index`."""
    for line in reversed(lines[:index]):
        stripped = line.strip()
        if stripped.startswith(BRANCH_PREFIXES):
            return stripped
    raise AssertionError(f"no enclosing branch found above line {index + 1}")


@pytest.mark.parametrize(("installer", "summary"), INSTALLERS)
def test_registered_summary_is_gated_on_the_registration_outcome(
    installer: Path, summary: str
) -> None:
    lines = installer.read_text(encoding="utf-8").splitlines()

    matches = [index for index, line in enumerate(lines) if summary in line]
    assert len(matches) == 1, f"expected exactly one summary line, found {len(matches)}"

    guard = _guard_for(lines, matches[0])
    assert guard == "if ($AutoStartRegistered) {", (
        f"{installer.name} prints {summary!r} under {guard!r}; it must be gated on "
        "the registration outcome, not on the -AutoStart request"
    )


@pytest.mark.parametrize(("installer", "summary"), INSTALLERS)
def test_registration_outcome_is_only_recorded_after_a_successful_call(
    installer: Path, summary: str
) -> None:
    lines = installer.read_text(encoding="utf-8").splitlines()
    text = "\n".join(lines)

    assert "$AutoStartRegistered = $false" in text, "the flag must start out false"

    successes = [
        index for index, line in enumerate(lines) if line.strip() == "$AutoStartRegistered = $true"
    ]
    assert len(successes) == 1, f"expected one success assignment, found {len(successes)}"

    # Only the statement directly after a returning Register-CuaDriverAutostart
    # call may flip the flag; anything else would re-introduce a summary that
    # does not depend on the outcome.
    previous = lines[successes[0] - 1].strip()
    assert previous.startswith("Register-CuaDriverAutostart"), (
        f"{installer.name} sets $AutoStartRegistered after {previous!r}; it must be set "
        "only after Register-CuaDriverAutostart returns without throwing"
    )


@pytest.mark.parametrize(("installer", "summary"), INSTALLERS)
def test_failed_registration_is_reported_in_the_summary(installer: Path, summary: str) -> None:
    text = installer.read_text(encoding="utf-8")

    assert "is NOT registered - registration failed above." in text, (
        f"{installer.name} must tell the user autostart is missing when -AutoStart was "
        "requested but registration failed"
    )
