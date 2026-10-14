"""Guards the encoding of the Cua Driver scripts served for remote execution.

The documented install/uninstall one-liners pipe a downloaded script straight
into an interpreter:

    irm https://cua.ai/driver/uninstall.ps1 | iex
    curl -fsSL https://cua.ai/driver/install.sh | sh

On that path a leading UTF-8 BOM is not consumed as an encoding signal. `irm`
decodes it into a literal U+FEFF at the head of the string, so `#` no longer
opens a comment on line 1 and PowerShell tokenizes the header comment as code.
That is the defect fixed in #3174, where the header text `(Windows)` was
evaluated as a command and the one-liner exited non-zero on a successful
uninstall.

Running the same file from disk hides the problem, because PowerShell treats a
leading BOM as a valid encoding signal when reading a script file. A BOM is
also invisible in review and in the GitHub diff view, and editors that default
to UTF-8-with-BOM on Windows can reintroduce one silently. Hence this guard.
"""

from __future__ import annotations

from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[3]
DRIVER_SCRIPTS = REPO_ROOT / "libs/cua-driver/scripts"

UTF8_BOM = b"\xef\xbb\xbf"

# Scripts published as release assets and fetched by the documented one-liners.
# Named explicitly so a rename or deletion fails loudly instead of quietly
# shrinking the globbed set below.
REMOTELY_EXECUTED_SCRIPTS = (
    "install.ps1",
    "uninstall.ps1",
    "install.sh",
    "uninstall.sh",
)


def driver_scripts() -> list[Path]:
    return sorted(
        path
        for pattern in ("*.ps1", "*.sh")
        for path in DRIVER_SCRIPTS.glob(pattern)
    )


@pytest.mark.parametrize("name", REMOTELY_EXECUTED_SCRIPTS)
def test_remotely_executed_script_exists(name: str):
    assert (DRIVER_SCRIPTS / name).is_file(), (
        f"{name} is missing from {DRIVER_SCRIPTS.relative_to(REPO_ROOT)}; "
        "update REMOTELY_EXECUTED_SCRIPTS if it was intentionally renamed"
    )


def test_driver_scripts_are_discovered():
    """A glob that matches nothing would make the BOM assertions vacuous."""
    discovered = {path.name for path in driver_scripts()}
    assert discovered, f"no scripts found under {DRIVER_SCRIPTS}"
    assert set(REMOTELY_EXECUTED_SCRIPTS) <= discovered


@pytest.mark.parametrize(
    "script", driver_scripts(), ids=lambda path: path.name
)
def test_driver_script_has_no_utf8_bom(script: Path):
    head = script.read_bytes()[: len(UTF8_BOM)]
    assert head != UTF8_BOM, (
        f"{script.relative_to(REPO_ROOT)} starts with a UTF-8 BOM. "
        "Rewrite it as UTF-8 without BOM: a BOM breaks the documented "
        "`irm ... | iex` and `curl ... | sh` one-liners (see #3174)."
    )
