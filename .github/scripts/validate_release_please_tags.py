#!/usr/bin/env python3
"""Reject Release Please manifest tags that are not on the target branch."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
from typing import Any, Mapping, Sequence


class TagValidationError(RuntimeError):
    """A released manifest version has an invalid Git tag."""


def package_tag(config: Mapping[str, Any], package: Mapping[str, Any], version: str) -> str:
    def option(name: str, default: Any) -> Any:
        return package.get(name, config.get(name, default))

    component = package.get("component", package.get("package-name"))
    if option("include-component-in-tag", False) and not component:
        raise TagValidationError("component tag requested without a component or package name")

    prefix = (
        str(component) + str(option("tag-separator", "-"))
        if option("include-component-in-tag", False)
        else ""
    )
    version_prefix = "v" if option("include-v-in-tag", True) else ""
    return f"{prefix}{version_prefix}{version}"


def git(*args: str, cwd: Path, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=check,
        capture_output=True,
        text=True,
    )


def validate_tags(
    *,
    repo_root: Path,
    config: Mapping[str, Any],
    manifest: Mapping[str, Any],
    target: str,
) -> list[str]:
    packages = config.get("packages")
    if not isinstance(packages, Mapping):
        raise TagValidationError("release config must contain a packages object")

    checked: list[str] = []
    for path, package in packages.items():
        if path not in manifest:
            raise TagValidationError(f"release manifest does not contain {path!r}")
        if not isinstance(package, Mapping):
            raise TagValidationError(f"release package config for {path!r} must be an object")

        tag = package_tag(config, package, str(manifest[path]))
        tag_ref = f"refs/tags/{tag}^{{commit}}"
        if git("rev-parse", "--verify", "--quiet", tag_ref, cwd=repo_root, check=False).returncode:
            # Immediately after a release PR merges, its new manifest version has
            # no tag yet. Release Please must be allowed to create that tag.
            continue
        if git(
            "merge-base", "--is-ancestor", tag_ref, target, cwd=repo_root, check=False
        ).returncode:
            tag_sha = git("rev-parse", tag_ref, cwd=repo_root).stdout.strip()
            target_sha = git("rev-parse", target, cwd=repo_root).stdout.strip()
            raise TagValidationError(
                f"manifest tag {tag} ({tag_sha}) for {path} is not an ancestor of "
                f"{target} ({target_sha}); refusing to generate release history"
            )
        checked.append(tag)
    return checked


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--config", type=Path, default=Path("release-please-config.json"))
    parser.add_argument("--manifest", type=Path, default=Path(".release-please-manifest.json"))
    parser.add_argument("--target", default="HEAD")
    args = parser.parse_args(argv)

    try:
        repo_root = args.repo_root.resolve()
        config_path = args.config if args.config.is_absolute() else repo_root / args.config
        manifest_path = args.manifest if args.manifest.is_absolute() else repo_root / args.manifest
        checked = validate_tags(
            repo_root=repo_root,
            config=json.loads(config_path.read_text()),
            manifest=json.loads(manifest_path.read_text()),
            target=args.target,
        )
    except (OSError, ValueError, subprocess.CalledProcessError, TagValidationError) as error:
        print(f"release tag validation error: {error}")
        return 1

    if checked:
        print(f"release manifest tags are on {args.target}: {', '.join(checked)}")
    else:
        print("release manifest tags do not exist yet; Release Please may create them")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
