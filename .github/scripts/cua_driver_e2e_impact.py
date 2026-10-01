"""Advise which Cua Driver desktop evidence a committed change may affect."""

import argparse
import json
import re
import subprocess
from pathlib import Path


PLATFORMS = ("linux", "windows", "macos")
PLATFORM_PREFIXES = {
    "linux": ("libs/cua-driver/rust/crates/platform-linux/", "scripts/ci/linux/"),
    "windows": ("libs/cua-driver/rust/crates/platform-windows/", "scripts/ci/windows/"),
    "macos": (
        "libs/cua-driver/rust/crates/platform-macos/",
        "scripts/ci/macos/",
        "libs/cua-driver/tests/runners/macos-lume/",
    ),
}
PLATFORM_WORKFLOWS = {
    ".github/workflows/e2e-rust-linux.yml": "linux",
    ".github/workflows/e2e-rust-windows.yml": "windows",
    ".github/workflows/e2e-rust-macos.yml": "macos",
}
DIAGNOSTIC_PATHS = {
    ".github/workflows/ci-cua-driver-quick.yml",
    ".github/workflows/ci-cua-driver-preflight.yml",
    ".github/scripts/cua_driver_e2e_impact.py",
    ".github/scripts/tests/test_cua_driver_e2e_impact.py",
    "scripts/ci/linux/preflight-rust-e2e.sh",
    "scripts/ci/windows/preflight-rust-e2e.ps1",
}
DOCUMENT_PATHS = {"README.md", "scripts/ci/README.md"}
DOCUMENT_PREFIXES = ("docs/", "libs/cua-driver/docs/")


def classify(paths: list[str]) -> dict:
    """Conservatively advise recertification; never waive the final matrix."""
    paths = sorted(set(paths))
    affected: set[str] = set()
    reasons: set[str] = set()
    for path in paths:
        if not path or path.startswith("/") or ".." in path.split("/"):
            affected.update(PLATFORMS)
            reasons.add("invalid or unrecognized path")
        elif path in DIAGNOSTIC_PATHS:
            reasons.add("non-certifying diagnostic tooling")
        elif path in PLATFORM_WORKFLOWS:
            affected.add(PLATFORM_WORKFLOWS[path])
            reasons.add("platform workflow or environment")
        elif any(path.startswith(prefix) for prefix in PLATFORM_PREFIXES["linux"]):
            affected.add("linux")
            reasons.add("platform implementation or harness")
        elif any(path.startswith(prefix) for prefix in PLATFORM_PREFIXES["windows"]):
            affected.add("windows")
            reasons.add("platform implementation or harness")
        elif any(path.startswith(prefix) for prefix in PLATFORM_PREFIXES["macos"]):
            affected.add("macos")
            reasons.add("platform implementation or harness")
        elif path in DOCUMENT_PATHS or (
            path.startswith(DOCUMENT_PREFIXES) and path.endswith((".md", ".mdx"))
        ):
            reasons.add("documentation only")
        else:
            affected.update(PLATFORMS)
            reasons.add("shared, executable, or unrecognized path")
    if not paths:
        affected.update(PLATFORMS)
        reasons.add("empty change set")
    return {
        "advisory_only": True,
        "affected_platforms": [platform for platform in PLATFORMS if platform in affected],
        "reasons": sorted(reasons),
        "changed_paths": paths,
        "note": (
            "A reviewer must account for the exact tested and candidate SHAs, "
            "final diff, and certification policy. This advice does not skip or certify tests."
        ),
    }


def changed_paths(repo: Path, tested_sha: str, candidate_sha: str) -> list[str]:
    for label, sha in (("tested", tested_sha), ("candidate", candidate_sha)):
        if not re.fullmatch(r"[0-9a-fA-F]{40}", sha):
            raise ValueError(f"{label} SHA must be a full 40-character commit hash")
        subprocess.run(
            ["git", "-C", str(repo), "cat-file", "-e", f"{sha}^{{commit}}"],
            check=True,
            stdout=subprocess.DEVNULL,
        )
    # Disabling rename detection includes both sides, including a removed
    # executable moved into an otherwise exempt documentation path.
    output = subprocess.check_output(
        [
            "git", "-C", str(repo), "diff", "--name-only", "-z", "--no-ext-diff",
            "--no-renames", tested_sha, candidate_sha, "--",
        ]
    )
    return [path.decode("utf-8", errors="surrogateescape") for path in output.split(b"\0") if path]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tested_sha", help="SHA that produced accepted evidence")
    parser.add_argument("candidate_sha", help="SHA to review for final delivery")
    args = parser.parse_args()
    try:
        paths = changed_paths(Path.cwd(), args.tested_sha, args.candidate_sha)
    except (ValueError, subprocess.CalledProcessError) as error:
        parser.error(str(error))
    print(json.dumps(classify(paths)))


if __name__ == "__main__":
    main()
