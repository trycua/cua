#!/usr/bin/env python3
"""Check a release commit's changelog against the PRs it carries, before it merges.

Release Please builds a release PR from main at one moment. A pull request that
merges after that, and before the release PR does, is in the tagged commit but
not in the immutable changelog, and the release workflow's attribution preflight
then fails after the tag exists (cua-driver-rs 0.47.20 and 0.41.5). This runs
the same `release_attribution.py collect` on a commit that has main's newest
state and the release PR's changes: the pull request head on a PR, and the merge
queue commit in the queue, where it fails the required ci-gate and ejects the
release PR. Release Please then rebuilds it with the missing entry.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

DRIVER_PATHS = [
    "--path", "libs/cua-driver",
    "--exclude-path", "libs/cua-driver/docs",
    "--exclude-path", "libs/cua-driver/examples",
    "--exclude-path", "libs/cua-driver/python",
    "--exclude-path", "libs/cua-driver/tests",
    "--exclude-path", "libs/cua-driver/rust/crates/cua-perception",
    "--exclude-path", "libs/cua-driver/experiments/cua-perception-inference",
    "--exclude-companion-path", "libs/cua-driver/rust/Cargo.toml",
    "--exclude-companion-path", "libs/cua-driver/rust/Cargo.lock",
    "--release-metadata-path", "libs/cua-driver/rust/CHANGELOG.md",
    "--release-metadata-path", "libs/cua-driver/rust/VERSION",
    "--release-metadata-path", "libs/cua-driver/rust/Cargo.toml",
    "--release-metadata-path", "libs/cua-driver/rust/Cargo.lock",
    "--release-metadata-path", "libs/cua-driver/rust/Skills/cua-driver/SKILL.md",
    "--release-metadata-path", "libs/cua-driver/typescript/package.json",
    "--release-metadata-path", "libs/cua-driver/typescript/package-lock.json",
]

# VERSION file -> (product, display name, tag prefix, changelog, extra collect arguments)
PRODUCTS = {
    "libs/cua-driver/rust/crates/cua-perception/VERSION": (
        "cua-perception",
        "Cua Perception",
        "cua-perception-v",
        "libs/cua-driver/rust/crates/cua-perception/CHANGELOG.md",
        ["--path", "libs/cua-driver/rust/crates/cua-perception"],
    ),
    "libs/cua-driver/rust/VERSION": (
        "cua-driver-rs",
        "Cua Driver",
        "cua-driver-rs-v",
        "libs/cua-driver/rust/CHANGELOG.md",
        DRIVER_PATHS,
    ),
    "libs/lume/VERSION": ("lume", "Lume", "lume-v", "libs/lume/CHANGELOG.md", ["--path", "libs/lume"]),
    "libs/python/cua-sandbox/VERSION": (
        "sandbox",
        "Cua Sandbox",
        "sandbox-v",
        "libs/python/cua-sandbox/CHANGELOG.md",
        ["--path", "libs/python/cua-sandbox"],
    ),
}


def changed_products(changed_files: list[str]) -> list[str]:
    """VERSION files the change touches, in a fixed order."""
    return [version_file for version_file in PRODUCTS if version_file in set(changed_files)]


def collect_args(
    version_file: str,
    *,
    version: str,
    repository: str,
    sha: str,
    output: Path,
    addenda: Path | None,
) -> list[str]:
    product, display, prefix, changelog, extra = PRODUCTS[version_file]
    args = [
        "--repository", repository,
        "--product", product,
        "--display-name", display,
        "--version", version,
        "--tag", f"{prefix}{version}",
        "--tag-prefix", prefix,
        "--ref", sha,
        "--sha", sha,
        *extra,
        "--changelog", changelog,
    ]
    if addenda is not None and addenda.is_file():
        args += ["--changelog-addenda", str(addenda)]
    return [*args, "--output", str(output)]


def run(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path("."))
    parser.add_argument("--control", type=Path, required=True, help="trusted checkout with release_attribution.py")
    parser.add_argument("--repository", required=True)
    parser.add_argument("--base-sha", required=True)
    parser.add_argument("--head-sha", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args(argv)

    diff = subprocess.run(
        ["git", "-C", str(args.repo_root), "diff", "--name-only", f"{args.base_sha}...{args.head_sha}"],
        check=True, capture_output=True, text=True,
    )
    version_files = changed_products(diff.stdout.split())
    if not version_files:
        print("No release version file changes; nothing to preflight.")
        return 0
    args.output_dir.mkdir(parents=True, exist_ok=True)
    failed = False
    for version_file in version_files:
        version = (args.repo_root / version_file).read_text().strip()
        output = args.output_dir / f"{PRODUCTS[version_file][0]}-release-manifest.json"
        command = [
            sys.executable,
            str(args.control / ".github/scripts/release_attribution.py"),
            "collect",
            "--repo-root", str(args.repo_root),
            "--config", str(args.control / ".github/release-attribution-config.json"),
            *collect_args(
                version_file,
                version=version,
                repository=args.repository,
                sha=args.head_sha,
                output=output,
                addenda=args.control / ".github/releases/changelog-addenda.json",
            ),
        ]
        result = subprocess.run(command)
        failed = failed or result.returncode != 0
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(run())
