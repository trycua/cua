"""Every ignored Cua Driver integration test has a runner or a written reason.

`#[ignore]` keeps a desktop-bound test out of plain `cargo test`. It then runs
only when a canonical runner or workflow selects it, either by name or through
a whole-binary `--ignored` run. A test that no runner selects silently stops
running; several regressions did exactly that before this inventory existed.
Tests that are deliberately manual must say why in the allowlist.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_ROOT = REPO_ROOT / "libs/cua-driver/rust/crates/cua-driver/tests"
ALLOWLIST = REPO_ROOT / "libs/cua-driver/tests/manual-e2e-allowlist.txt"

# Canonical runners and the workflows that call them. The Windows Sandbox
# runner is documented as non-canonical and cannot certify a test.
RUNNER_GLOBS = (
    ".github/workflows/*.yml",
    "scripts/ci/**/*.sh",
    "scripts/ci/**/*.ps1",
    "libs/cua-driver/tests/runners/macos-lume/*.sh",
    "libs/cua-driver/tests/runners/windows/*.ps1",
)

IDENT = r"[A-Za-z_][A-Za-z0-9_]*"
FN_RE = re.compile(rf"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\$?{IDENT})")
MACRO_DEF_RE = re.compile(rf"macro_rules!\s*({IDENT})")
PATH_MOD_RE = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
# The next command in a runner ends the current command's argument list.
COMMAND_BOUNDARY_RE = re.compile(r"\bcargo\s|Invoke-CargoTest\b|\brun_test\s|\n\s*\n")


def _ignored_in_source(text: str) -> list[str]:
    """Return ignored test names, expanding ignored `macro_rules!` rows."""
    lines = text.splitlines()
    names: list[str] = []
    ignored_macros: set[str] = set()
    for index, line in enumerate(lines):
        if not line.lstrip().startswith("#[ignore"):
            continue
        for following in lines[index + 1 :]:
            match = FN_RE.match(following)
            if match:
                break
        else:
            raise AssertionError(f"#[ignore] without a following fn: {line!r}")
        name = match.group(1)
        if not name.startswith("$"):
            names.append(name)
            continue
        definitions = MACRO_DEF_RE.findall("\n".join(lines[: index + 1]))
        assert definitions, f"macro-generated ignored test outside macro_rules: {line!r}"
        ignored_macros.add(definitions[-1])
    for macro in sorted(ignored_macros):
        rows = re.findall(rf"(?<![A-Za-z0-9_]){macro}!\s*\(\s*({IDENT})", text)
        assert rows, f"ignored macro {macro} has no invocations"
        names.extend(rows)
    return names


def _test_binary_sources() -> dict[str, list[Path]]:
    """Map each integration-test binary to the source files it compiles."""
    binaries: dict[str, list[Path]] = {}
    for path in sorted(TEST_ROOT.glob("*.rs")):
        sources = [path]
        for relative in PATH_MOD_RE.findall(path.read_text(encoding="utf-8")):
            sources.append((path.parent / relative).resolve())
        binaries[path.stem] = sources
    return binaries


def ignored_tests() -> set[tuple[str, str]]:
    found: set[tuple[str, str]] = set()
    for binary, sources in _test_binary_sources().items():
        for source in sources:
            for name in _ignored_in_source(source.read_text(encoding="utf-8")):
                found.add((binary, name))
    return found


def runner_texts() -> dict[str, str]:
    texts: dict[str, str] = {}
    for pattern in RUNNER_GLOBS:
        for path in sorted(REPO_ROOT.glob(pattern)):
            texts[str(path.relative_to(REPO_ROOT))] = path.read_text(encoding="utf-8")
    return texts


def _word(name: str) -> re.Pattern[str]:
    return re.compile(rf"(?<![A-Za-z0-9_]){re.escape(name)}(?![A-Za-z0-9_])")


def runs_whole_binary_ignored(text: str, binary: str) -> bool:
    """True when some command runs every ignored test in `binary`."""
    for match in re.finditer(rf"--test\W+{re.escape(binary)}(?![A-Za-z0-9_])", text):
        rest = text[match.end() :]
        boundary = COMMAND_BOUNDARY_RE.search(rest)
        command = rest[: boundary.start()] if boundary else rest
        if "--ignored" in command and "--exact" not in command:
            return True
    return False


def selecting_runners(binary: str, name: str, texts: dict[str, str]) -> list[str]:
    pattern = _word(name)
    return [
        path
        for path, text in texts.items()
        if pattern.search(text) or runs_whole_binary_ignored(text, binary)
    ]


def load_allowlist() -> dict[tuple[str, str], str]:
    entries: dict[tuple[str, str], str] = {}
    for number, raw in enumerate(ALLOWLIST.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        test, separator, reason = line.partition("|")
        binary, colons, name = test.strip().partition("::")
        assert separator and colons and binary and name, (
            f"{ALLOWLIST.name}:{number}: expected '<binary>::<test> | <reason>'"
        )
        assert reason.strip(), f"{ALLOWLIST.name}:{number}: missing reason"
        key = (binary, name.strip())
        assert key not in entries, f"{ALLOWLIST.name}:{number}: duplicate {test.strip()}"
        entries[key] = reason.strip()
    return entries


def test_every_ignored_test_is_routed_or_allowlisted() -> None:
    texts = runner_texts()
    allowlist = load_allowlist()
    orphans = sorted(
        f"{binary}::{name}"
        for binary, name in ignored_tests()
        if (binary, name) not in allowlist and not selecting_runners(binary, name, texts)
    )
    assert not orphans, (
        "These #[ignore] tests are never selected by a canonical runner. Route "
        f"them or add them with a reason to {ALLOWLIST.relative_to(REPO_ROOT)}:\n  "
        + "\n  ".join(orphans)
    )


def test_allowlist_entries_are_live_and_still_manual() -> None:
    texts = runner_texts()
    ignored = ignored_tests()
    stale = []
    routed = []
    for binary, name in sorted(load_allowlist()):
        if (binary, name) not in ignored:
            stale.append(f"{binary}::{name}")
        elif runners := selecting_runners(binary, name, texts):
            routed.append(f"{binary}::{name} ({', '.join(runners)})")
    assert not stale, "Allowlisted tests are no longer ignored tests:\n  " + "\n  ".join(stale)
    assert not routed, "Allowlisted tests are routed; drop them:\n  " + "\n  ".join(routed)


def test_inventory_expands_macro_rows_and_support_modules() -> None:
    ignored = ignored_tests()
    assert ("harness_gtk3_test", "harness_gtk3_left_click_ax_background") in ignored
    assert ("standalone_browser_behavior_test", "standalone_browser_roundtrip") in ignored
    assert (
        "harness_appkit_test",
        "harness_appkit_pending_snapshot_cannot_retarget_token",
    ) in ignored


@pytest.mark.parametrize(
    ("runner", "selected"),
    [
        ("cargo test --test demo_test -- --ignored --nocapture\n", True),
        ('"--test", "demo_test", "--",\n    "--ignored", "--nocapture"\n', True),
        ("cargo test --test demo_test -- --ignored --exact other\n", False),
        ("cargo test --test demo_test -- --nocapture\ncargo test --ignored\n", False),
        ("cargo test --test demo_test_extra -- --ignored\n", False),
    ],
)
def test_whole_binary_detection_is_scoped_to_one_command(runner: str, selected: bool) -> None:
    assert runs_whole_binary_ignored(runner, "demo_test") is selected
