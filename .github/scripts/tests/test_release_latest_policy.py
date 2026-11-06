"""GitHub has one repo-wide "Latest" release and trycua/cua publishes many
packages. Nothing depends on it (test_no_latest_consumers.py guards that), so
which release carries the badge is cosmetic. We pin it to Cua Spaces, the app
people browsing the repository expect; every other release workflow opts out
explicitly so the badge does not flip between packages."""

import re
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = REPO_ROOT / ".github" / "workflows"
LATEST_OWNER = "cd-cua-spaces.yml"
THIS = Path(__file__).resolve()


def _tracked_text_files():
    out = subprocess.run(
        ["git", "ls-files", "-z"], cwd=REPO_ROOT, check=True, capture_output=True, text=True
    ).stdout
    for rel in filter(None, out.split("\0")):
        path = REPO_ROOT / rel
        if (
            path == THIS
            or rel.startswith(".github/release-backfill/")
            or rel.endswith((".md", ".mdx", ".lock", ".json"))
            or "/tests/" in rel
            or rel == ".github/scripts/github_release.py"  # verifies only on --make-latest, which no workflow passes
            or not path.is_file()
        ):
            continue
        try:
            yield rel, path.read_text()
        except (UnicodeDecodeError, OSError):
            continue


def test_publishing_workflows_set_latest_explicitly():
    problems = []
    for wf in sorted(WORKFLOWS.glob("*.yml")):
        text = wf.read_text()
        publishes = (
            re.search(r"gh release edit[^\n]*--draft=false", text)
            or re.search(r'gh release edit[^\n]*"\$\{(flags|FLAGS)\[@\]\}"', text)
            or "softprops/action-gh-release" in text
        )
        if publishes and not re.search(r"--latest|make_latest", text):
            problems.append(f"{wf.name}: publishes a release without --latest/make_latest")
        if "github_release.py" in text and "--make-latest" in text:
            problems.append(f"{wf.name}: passes --make-latest")
        if wf.name != LATEST_OWNER:
            for m in re.finditer(r"(?:gh release|flags\+=|FLAGS\+=|--draft=false)[^\n]*--latest(?!=false)\b", text):
                problems.append(f"{wf.name}: --latest without =false (only {LATEST_OWNER} may)")
            for m in re.finditer(r"make_latest:\s*[\"']?(\w+)", text):
                if m.group(1) != "false":
                    problems.append(f"{wf.name}: make_latest must be false")
    assert not problems, "\n".join(problems)


def test_cua_spaces_is_the_only_latest_owner():
    text = (WORKFLOWS / LATEST_OWNER).read_text()
    assert "flags+=(--latest)" in text


def test_no_repo_wide_latest_lookup_for_trycua_cua():
    bad = []
    pattern = re.compile(
        r"(trycua/cua|\$GITHUB_REPO\b|\$\{GITHUB_REPO\}|\{repository\}|GITHUB_REPOSITORY)[^\n]*releases/latest"
        r"|releases/latest[^\n]*(trycua/cua)"
    )
    for rel, text in _tracked_text_files():
        for n, line in enumerate(text.splitlines(), 1):
            if pattern.search(line) or re.search(r"gh release view\s*(--|$)", line):
                bad.append(f"{rel}:{n}: {line.strip()[:120]}")
    assert not bad, "repo-wide Latest lookups; filter by tag prefix instead:\n" + "\n".join(bad)


def test_publishing_workflows_run_the_latest_pin_after_publishing():
    # --latest=false is not enough: publishing a draft handed cua-spacesd 0.12.1
    # and 0.12.2 the repo-wide Latest anyway. Every workflow that publishes a
    # release draft re-pins Latest right after.
    missing = []
    for wf in sorted(WORKFLOWS.glob("*.yml")):
        text = wf.read_text()
        if wf.name == LATEST_OWNER or wf.name == "release-latest-guard.yml":
            continue
        if re.search(r"gh release edit[^\n]*--draft=false", text) or re.search(
            r'gh release edit[^\n]*"\$\{(flags|FLAGS)\[@\]\}"', text
        ):
            if "pin_latest_release.py" not in text:
                missing.append(wf.name)
    assert not missing, f"publish without re-pinning Latest: {missing}"
