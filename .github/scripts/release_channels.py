#!/usr/bin/env python3
"""Plan and stage immutable component releases without changing stable state."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Any, Mapping, Sequence

import release_attribution


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_REGISTRY = ROOT / ".github/releases/components.json"
SEMVER_PATTERN = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
SEMVER_RE = re.compile(rf"^{SEMVER_PATTERN}$")
NIGHTLY_VERSION_RE = re.compile(
    rf"^(?P<base>{SEMVER_PATTERN})-nightly\."
    r"(?P<date>[0-9]{8})\.(?P<run>[1-9][0-9]*)$"
)
SHA_RE = re.compile(r"^[0-9a-f]{40}$")


class ChannelError(RuntimeError):
    """A release-channel invariant failed."""


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ChannelError(f"cannot read JSON from {path}: {error}") from error


def repository_path(root: Path, value: str) -> Path:
    pure = PurePosixPath(value)
    if pure.is_absolute() or ".." in pure.parts:
        raise ChannelError(f"repository path must be relative and confined: {value!r}")
    resolved = (root / pure).resolve()
    try:
        resolved.relative_to(root.resolve())
    except ValueError as error:
        raise ChannelError(f"repository path escapes the checkout: {value!r}") from error
    return resolved


def load_registry(
    path: Path = DEFAULT_REGISTRY,
    *,
    root: Path = ROOT,
) -> dict[str, Any]:
    registry = read_json(path)
    if registry.get("schemaVersion") != 1:
        raise ChannelError("component registry schemaVersion must be 1")
    components = registry.get("components")
    if not isinstance(components, dict) or not components:
        raise ChannelError("component registry must contain components")
    shared = registry.get("sharedChangePaths")
    if not isinstance(shared, list):
        raise ChannelError("component registry sharedChangePaths must be a list")

    prefixes: dict[str, str] = {}
    for name, component in components.items():
        if not isinstance(component, dict):
            raise ChannelError(f"component {name} must be an object")
        for field in (
            "releasePleasePath",
            "displayName",
            "stableTagPrefix",
            "nightlyTagPrefix",
            "versionAuthorityFile",
            "changelog",
            "builderWorkflow",
            "changeDetectionPaths",
            "buildVersionSites",
            "channels",
        ):
            if field not in component:
                raise ChannelError(f"component {name} is missing {field}")
        if component["stableTagPrefix"] == component["nightlyTagPrefix"]:
            raise ChannelError(f"component {name} has overlapping channel prefixes")
        if not str(component["nightlyTagPrefix"]).startswith("nightly-"):
            raise ChannelError(f"component {name} nightlyTagPrefix must start with nightly-")
        for channel in ("stableTagPrefix", "nightlyTagPrefix"):
            prefix = str(component[channel])
            if prefix in prefixes:
                raise ChannelError(
                    f"tag prefix {prefix!r} is shared by {prefixes[prefix]} and {name}"
                )
            prefixes[prefix] = name
        for field in ("versionAuthorityFile", "changelog", "builderWorkflow"):
            if not repository_path(root, str(component[field])).is_file():
                raise ChannelError(f"component {name} {field} does not exist: {component[field]}")
        sites = component["buildVersionSites"]
        if not isinstance(sites, list) or not sites:
            raise ChannelError(f"component {name} must declare buildVersionSites")
        for site in sites:
            if not repository_path(root, str(site["path"])).is_file():
                raise ChannelError(f"component {name} version site does not exist: {site['path']}")
        channels = component["channels"]
        if channels != {"nightly": True, "registries": []}:
            raise ChannelError(
                f"component {name} v1 channels must enable nightly and declare no registries"
            )

    release_config = read_json(root / "release-please-config.json")
    release_manifest = read_json(root / ".release-please-manifest.json")
    packages = release_config.get("packages", {})
    for name, component in components.items():
        package_path = component["releasePleasePath"]
        package = packages.get(package_path)
        if not isinstance(package, dict):
            raise ChannelError(f"component {name} has no Release Please package {package_path}")
        if package.get("component") != name:
            raise ChannelError(
                f"component {name} does not match Release Please component {package.get('component')!r}"
            )
        expected_prefix = f"{name}-v"
        if component["stableTagPrefix"] != expected_prefix:
            raise ChannelError(
                f"component {name} stable prefix must match Release Please: {expected_prefix}"
            )
        authority = (
            repository_path(root, component["versionAuthorityFile"])
            .read_text(encoding="utf-8")
            .strip()
        )
        if not SEMVER_RE.fullmatch(authority):
            raise ChannelError(f"component {name} authority is not a stable version: {authority!r}")
        if release_manifest.get(package_path) != authority:
            raise ChannelError(
                f"component {name} authority {authority} differs from Release Please manifest "
                f"{release_manifest.get(package_path)!r}"
            )
    return registry


def component_descriptor(
    name: str,
    path: Path = DEFAULT_REGISTRY,
    *,
    root: Path = ROOT,
) -> dict[str, Any]:
    registry = load_registry(path, root=root)
    try:
        return dict(registry["components"][name])
    except KeyError as error:
        raise ChannelError(f"unknown release component: {name}") from error


def stable_version(value: str) -> tuple[int, int, int]:
    if not SEMVER_RE.fullmatch(value):
        raise ChannelError(f"invalid stable version: {value!r}")
    return tuple(int(part) for part in value.split("."))  # type: ignore[return-value]


def nightly_version(value: str) -> re.Match[str]:
    match = NIGHTLY_VERSION_RE.fullmatch(value)
    if not match:
        raise ChannelError(f"invalid nightly version: {value!r}")
    try:
        datetime.strptime(match.group("date"), "%Y%m%d")
    except ValueError as error:
        raise ChannelError(f"invalid nightly date in version: {value!r}") from error
    return match


def derive_nightly_version(base: str, date: str, run: str) -> str:
    major, minor, patch = stable_version(base)
    candidate = f"{major}.{minor}.{patch + 1}-nightly.{date}.{run}"
    nightly_version(candidate)
    return candidate


def format_tag(component: Mapping[str, Any], channel: str, version: str) -> str:
    if channel == "stable":
        stable_version(version)
        return f"{component['stableTagPrefix']}{version}"
    if channel == "nightly":
        nightly_version(version)
        return f"{component['nightlyTagPrefix']}{version}"
    raise ChannelError(f"unsupported channel: {channel}")


def parse_tag(component: Mapping[str, Any], channel: str, tag: str) -> str:
    prefix_key = "stableTagPrefix" if channel == "stable" else "nightlyTagPrefix"
    if channel not in {"stable", "nightly"}:
        raise ChannelError(f"unsupported channel: {channel}")
    prefix = str(component[prefix_key])
    if not tag.startswith(prefix):
        raise ChannelError(f"tag {tag!r} is outside the {channel} namespace {prefix!r}")
    version = tag[len(prefix) :]
    if channel == "stable":
        stable_version(version)
    else:
        nightly_version(version)
    if format_tag(component, channel, version) != tag:
        raise ChannelError(f"tag {tag!r} is not canonical")
    return version


def _workspace_package_names(manifest_path: Path) -> set[str]:
    manifest = manifest_path.read_text(encoding="utf-8")
    members_match = re.search(r"(?ms)^members\s*=\s*\[(.*?)\]", manifest)
    if not members_match:
        raise ChannelError(f"Cargo workspace members are missing from {manifest_path}")
    members = re.findall(r'"([^"]+)"', members_match.group(1))
    names: set[str] = set()
    for pattern in members:
        for member in manifest_path.parent.glob(str(pattern)):
            package_manifest = member / "Cargo.toml"
            if package_manifest.is_file():
                package_text = package_manifest.read_text(encoding="utf-8")
                package_block = re.search(r"(?ms)^\[package\]\s*(.*?)(?=^\[|\Z)", package_text)
                if not package_block or not re.search(
                    r"(?m)^version\.workspace\s*=\s*true\s*$", package_block.group(1)
                ):
                    continue
                name_match = re.search(r'(?m)^name\s*=\s*"([^"]+)"\s*$', package_block.group(1))
                if not name_match:
                    raise ChannelError(f"Cargo package name is missing from {package_manifest}")
                names.add(name_match.group(1))
    if not names:
        raise ChannelError(
            f"no version-inheriting Cargo workspace packages found in {manifest_path}"
        )
    return names


def _rewrite_cargo_lock(
    lock_path: Path, manifest_path: Path, old_version: str, new_version: str
) -> int:
    names = _workspace_package_names(manifest_path)
    original = lock_path.read_text(encoding="utf-8")
    seen: set[str] = set()

    def replace_block(match: re.Match[str]) -> str:
        block = match.group(0)
        name_match = re.search(r'(?m)^name = "([^"]+)"$', block)
        version_match = re.search(r'(?m)^version = "([^"]+)"$', block)
        if not name_match or not version_match or name_match.group(1) not in names:
            return block
        if re.search(r"(?m)^source = ", block):
            raise ChannelError(f"workspace package {name_match.group(1)} unexpectedly has a source")
        if version_match.group(1) != old_version:
            raise ChannelError(
                f"workspace package {name_match.group(1)} has lock version "
                f"{version_match.group(1)}, expected {old_version}"
            )
        seen.add(name_match.group(1))
        start, end = version_match.span(1)
        return f"{block[:start]}{new_version}{block[end:]}"

    rewritten = re.sub(
        r"(?ms)^\[\[package\]\]\n.*?(?=^\[\[package\]\]\n|\Z)", replace_block, original
    )
    missing = names - seen
    if missing:
        raise ChannelError(
            f"Cargo.lock is missing workspace packages: {', '.join(sorted(missing))}"
        )
    lock_path.write_text(rewritten, encoding="utf-8")
    return len(seen)


def _rewrite_version_site(
    site: Mapping[str, Any],
    path: Path,
    old_version: str,
    new_version: str,
    *,
    manifest_path: Path | None = None,
) -> None:
    kind = site["kind"]
    if kind == "plain":
        if path.read_text(encoding="utf-8").strip() != old_version:
            raise ChannelError(f"plain version site {site['path']} differs from {old_version}")
        path.write_text(f"{new_version}\n", encoding="utf-8")
    elif kind == "regex":
        original = path.read_text(encoding="utf-8")
        replacement = str(site["replacement"]).replace("{version}", new_version)
        rewritten, count = re.subn(str(site["pattern"]), replacement, original)
        if count != int(site["expectedMatches"]):
            raise ChannelError(
                f"version site {site['path']} matched {count} times, "
                f"expected {site['expectedMatches']}"
            )
        path.write_text(rewritten, encoding="utf-8")
    elif kind == "cargo-workspace-lock":
        if manifest_path is None:
            raise ChannelError(
                f"cargo lock version site {site['path']} requires its manifest in the staged tree"
            )
        _rewrite_cargo_lock(path, manifest_path, old_version, new_version)
    else:
        raise ChannelError(f"unsupported version site kind: {kind!r}")


def apply_version(
    name: str,
    version: str,
    *,
    registry_path: Path = DEFAULT_REGISTRY,
    root: Path = ROOT,
) -> list[str]:
    nightly_version(version)
    component = component_descriptor(name, registry_path, root=root)
    authority = repository_path(root, component["versionAuthorityFile"])
    old_version = authority.read_text(encoding="utf-8").strip()
    stable_version(old_version)
    changed: list[str] = []
    for site in component["buildVersionSites"]:
        path = repository_path(root, site["path"])
        manifest = (
            repository_path(root, site["manifestPath"])
            if site["kind"] == "cargo-workspace-lock"
            else None
        )
        _rewrite_version_site(site, path, old_version, version, manifest_path=manifest)
        changed.append(str(site["path"]))
    return changed


def stage_versioned_tree(
    name: str,
    version: str,
    source: str,
    destination: str,
    *,
    registry_path: Path = DEFAULT_REGISTRY,
    root: Path = ROOT,
) -> list[str]:
    """Copy one source tree and rewrite only its declared version sites."""
    nightly_version(version)
    component = component_descriptor(name, registry_path, root=root)
    authority = repository_path(root, component["versionAuthorityFile"])
    old_version = authority.read_text(encoding="utf-8").strip()
    stable_version(old_version)
    source_path = repository_path(root, source)
    destination_path = repository_path(root, destination)
    if not source_path.is_dir():
        raise ChannelError(f"versioned tree source is not a directory: {source}")
    if destination_path.exists():
        raise ChannelError(f"versioned tree destination already exists: {destination}")
    if destination_path.resolve().is_relative_to(source_path.resolve()):
        raise ChannelError("versioned tree destination cannot be inside its source")

    source_pure = PurePosixPath(source)
    selected: list[tuple[Mapping[str, Any], PurePosixPath]] = []
    for site in component["buildVersionSites"]:
        try:
            relative = PurePosixPath(str(site["path"])).relative_to(source_pure)
        except ValueError:
            continue
        selected.append((site, relative))
    if not selected:
        raise ChannelError(
            f"versioned tree {source!r} contains no declared buildVersionSites for {name}"
        )

    destination_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        dir=destination_path.parent,
        prefix=f".{destination_path.name}.stage-",
    ) as temporary:
        staged = Path(temporary) / destination_path.name
        shutil.copytree(source_path, staged)
        changed: list[str] = []
        for site, relative in selected:
            manifest = None
            if site["kind"] == "cargo-workspace-lock":
                try:
                    manifest_relative = PurePosixPath(str(site["manifestPath"])).relative_to(
                        source_pure
                    )
                except ValueError as error:
                    raise ChannelError(
                        f"cargo lock version site {site['path']} cannot be staged without "
                        f"manifest {site['manifestPath']}"
                    ) from error
                manifest = staged / manifest_relative
            _rewrite_version_site(
                site,
                staged / relative,
                old_version,
                version,
                manifest_path=manifest,
            )
            changed.append(str(PurePosixPath(destination) / relative))
        staged.rename(destination_path)
    return changed


def _git(root: Path, *args: str) -> str:
    result = subprocess.run(["git", *args], cwd=root, check=False, text=True, capture_output=True)
    if result.returncode != 0:
        raise ChannelError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def nightly_attribution_preflight(
    *,
    root: Path,
    previous_tag: str | None,
    source_ref: str,
    paths: Sequence[str],
    config_path: Path,
) -> list[dict[str, str]]:
    """Find unresolved squash-generated coauthors before an expensive build."""
    try:
        config = read_json(config_path)
        commits = release_attribution.commits_in_range(root, previous_tag, source_ref, paths)
        return release_attribution.unresolved_coauthor_identities(commits, config)
    except release_attribution.ReleaseError as error:
        raise ChannelError(f"nightly attribution preflight failed: {error}") from error


def plan_nightly(
    name: str,
    source_sha: str,
    date: str,
    run: str,
    releases: Sequence[Mapping[str, Any]],
    *,
    force: bool = False,
    registry_path: Path = DEFAULT_REGISTRY,
    root: Path = ROOT,
    attribution_config_path: Path | None = None,
) -> dict[str, Any]:
    if not SHA_RE.fullmatch(source_sha):
        raise ChannelError(f"source SHA must be 40 lowercase hex characters: {source_sha!r}")
    component = component_descriptor(name, registry_path, root=root)
    base = (
        repository_path(root, component["versionAuthorityFile"]).read_text(encoding="utf-8").strip()
    )
    version = derive_nightly_version(base, date, run)
    tag = format_tag(component, "nightly", version)
    candidates: list[Mapping[str, Any]] = []
    for release in releases:
        if release.get("draft"):
            continue
        candidate_tag = str(release.get("tag_name", ""))
        try:
            parse_tag(component, "nightly", candidate_tag)
        except ChannelError:
            continue
        candidates.append(release)
    candidates.sort(
        key=lambda value: str(value.get("published_at") or value.get("created_at") or "")
    )
    previous_tag = str(candidates[-1]["tag_name"]) if candidates else None
    previous_sha = _git(root, "rev-list", "-n", "1", previous_tag) if previous_tag else None
    attribution_base_tag = previous_tag or format_tag(component, "stable", base)
    attribution_base_sha = previous_sha or _git(root, "rev-list", "-n", "1", attribution_base_tag)
    if not attribution_base_sha:
        raise ChannelError(f"attribution base tag has no commit: {attribution_base_tag}")
    _git(root, "merge-base", "--is-ancestor", attribution_base_sha, source_sha)
    paths = list(component["changeDetectionPaths"])
    paths.extend(load_registry(registry_path, root=root)["sharedChangePaths"])
    paths = sorted(set(str(path) for path in paths))
    if force:
        should_build, reason = True, "forced"
    elif previous_sha is None:
        should_build, reason = True, "first-nightly"
    elif previous_sha == source_sha:
        should_build, reason = False, "source-unchanged"
    else:
        changed = _git(root, "diff", "--name-only", previous_sha, source_sha, "--", *paths)
        should_build = bool(changed)
        reason = "relevant-changes" if should_build else "component-unchanged"
    attribution_issues: list[dict[str, str]] = []
    if should_build and attribution_config_path is not None:
        attribution_issues = nightly_attribution_preflight(
            root=root,
            previous_tag=attribution_base_tag,
            source_ref=source_sha,
            paths=paths,
            config_path=attribution_config_path,
        )
        if attribution_issues:
            should_build = False
            reason = "held-attribution"
    return {
        "component": name,
        "channel": "nightly",
        "version": version,
        "bundleVersion": version.split("-", 1)[0],
        "tag": tag,
        "sourceSha": source_sha,
        "previousTag": previous_tag,
        "previousNightlyTag": previous_tag,
        "previousSha": previous_sha,
        "attributionBaseTag": attribution_base_tag,
        "attributionBaseSha": attribution_base_sha,
        "shouldBuild": should_build,
        "reason": reason,
        "attributionIssues": attribution_issues,
    }


def build_manifest(
    name: str,
    version: str,
    tag: str,
    source_sha: str,
    previous_tag: str | None,
    asset_dir: Path,
    *,
    repository: str,
    registry_path: Path = DEFAULT_REGISTRY,
    root: Path = ROOT,
    attribution_config_path: Path | None = None,
    github: release_attribution.GitHubClient | None = None,
) -> dict[str, Any]:
    component = component_descriptor(name, registry_path, root=root)
    parsed = parse_tag(component, "nightly", tag)
    if parsed != version:
        raise ChannelError(f"tag version {parsed} differs from requested version {version}")
    if not SHA_RE.fullmatch(source_sha):
        raise ChannelError(f"invalid source SHA: {source_sha!r}")
    if not previous_tag:
        raise ChannelError("nightly attribution requires a bounded previous tag")
    if not asset_dir.is_dir() or not any(path.is_file() for path in asset_dir.rglob("*")):
        raise ChannelError(f"asset directory {asset_dir} is empty")
    base_sha = _git(root, "rev-list", "-n", "1", previous_tag)
    if not base_sha:
        raise ChannelError(f"nightly attribution base has no commit: {previous_tag}")
    _git(root, "merge-base", "--is-ancestor", base_sha, source_sha)
    registry = load_registry(registry_path, root=root)
    paths = sorted(
        {
            *(str(path) for path in component["changeDetectionPaths"]),
            *(str(path) for path in registry["sharedChangePaths"]),
        }
    )
    config_path = attribution_config_path or root / ".github/release-attribution-config.json"
    client = github or release_attribution.GitHubClient(
        os.environ.get("GH_TOKEN", ""),
        os.environ.get("GITHUB_API_URL", "https://api.github.com"),
    )
    try:
        return release_attribution.build_manifest(
            repo_root=root,
            repository=repository,
            product=name,
            display_name=str(component["displayName"]),
            version=version,
            tag=tag,
            previous_tag=previous_tag,
            expected_sha=source_sha,
            paths=paths,
            changelog_path=repository_path(root, component["changelog"]),
            attribution_config=read_json(config_path),
            github=client,
            release_ref=source_sha,
            asset_dir=asset_dir,
            channel="nightly",
        )
    except release_attribution.ReleaseError as error:
        raise ChannelError(str(error)) from error


def render_nightly_body(manifest: Mapping[str, Any]) -> str:
    if manifest.get("channel") != "nightly":
        raise ChannelError("nightly body rendering requires a nightly manifest")
    product = str(manifest["product"])
    version = str(manifest["version"])
    tag = str(manifest["tag"])
    source_sha = str(manifest["sha"])
    repository = str(manifest["repository"])
    if product == "cua-driver-rs":
        install = (
            f"CUA_DRIVER_RS_VERSION={tag} "
            '/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"'
        )
    elif product == "lume":
        install = f"curl -fsSL https://cua.ai/lume/install.sh | LUME_VERSION={tag} bash"
    else:
        raise ChannelError(f"no exact nightly installation contract for {product}")
    lines = [
        f"# {manifest['displayName']} {version} (nightly)",
        "",
        "Automated immutable nightly built from an exact `main` commit. Nightlies are",
        "opt-in, may be less stable than regular releases, and never replace stable",
        "update discovery.",
        "",
        "## Source",
        "",
        f"- Commit: [`{source_sha}`](https://github.com/{repository}/commit/{source_sha})",
        f"- Changes: [{manifest['previousTag'] or 'first nightly'}…`{source_sha[:12]}`]({manifest['compareUrl']})",
    ]
    changes = list(manifest["changes"])
    lines.extend(["", "## Changes", ""])
    if changes:
        for change in changes:
            label = str(change["type"]).replace("-", " ").title()
            suffix = release_attribution.format_change_thanks(change)
            bullet = (
                f"- **{label}:** {change['summary']}. "
                f"([#{change['pr']}](https://github.com/{repository}/pull/{change['pr']}))"
            )
            if suffix:
                bullet += f" {suffix}."
            lines.append(bullet)
    else:
        lines.append("No conventional pull-request changes matched this component's paths.")

    external = [item for item in manifest["contributors"] if item.get("external")]
    lines.extend(["", "## Contributors", ""])
    if external:
        lines.append(
            "Thanks to "
            + ", ".join(f"@{item['login']}" for item in external)
            + " for contributing to or reporting changes in this nightly."
        )
    else:
        lines.append("This nightly contains maintainer changes only.")

    lines.extend(
        [
            "",
            "## Exact installation",
            "",
            "```bash",
            install,
            "```",
            "",
            "## SHA256 checksums",
            "",
            "```text",
        ]
    )
    lines.extend(f"{asset['sha256']}  {asset['name']}" for asset in manifest["assets"])
    lines.extend(["```", ""])
    return "\n".join(lines)


def write_github_outputs(values: Mapping[str, Any], path: Path) -> None:
    lines = []
    for key, value in values.items():
        rendered = (
            json.dumps(value, separators=(",", ":"))
            if isinstance(value, (list, dict))
            else str(value)
        )
        if isinstance(value, bool):
            rendered = str(value).lower()
        if value is None:
            rendered = ""
        lines.append(f"{key}={rendered}")
    with path.open("a", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--registry", type=Path, default=DEFAULT_REGISTRY)
    subparsers = result.add_subparsers(dest="command", required=True)
    subparsers.add_parser("validate")

    derive = subparsers.add_parser("derive")
    derive.add_argument("--component", required=True)
    derive.add_argument("--date", default=datetime.now(timezone.utc).strftime("%Y%m%d"))
    derive.add_argument("--run", required=True)

    parse = subparsers.add_parser("parse-tag")
    parse.add_argument("--component", required=True)
    parse.add_argument("--channel", choices=("stable", "nightly"), required=True)
    parse.add_argument("--tag", required=True)

    apply = subparsers.add_parser("apply-version")
    apply.add_argument("--component", required=True)
    apply.add_argument("--version", required=True)

    stage = subparsers.add_parser("stage-versioned-tree")
    stage.add_argument("--component", required=True)
    stage.add_argument("--version", required=True)
    stage.add_argument("--source", required=True)
    stage.add_argument("--destination", required=True)

    plan = subparsers.add_parser("plan")
    plan.add_argument("--component", required=True)
    plan.add_argument("--source-sha", required=True)
    plan.add_argument("--date", required=True)
    plan.add_argument("--run", required=True)
    plan.add_argument("--releases", type=Path, required=True)
    plan.add_argument("--force", action="store_true")
    plan.add_argument("--attribution-config", type=Path)
    plan.add_argument("--output", type=Path)
    plan.add_argument("--github-output", type=Path)

    manifest = subparsers.add_parser("manifest")
    manifest.add_argument("--component", required=True)
    manifest.add_argument("--version", required=True)
    manifest.add_argument("--tag", required=True)
    manifest.add_argument("--source-sha", required=True)
    manifest.add_argument("--previous-tag")
    manifest.add_argument("--asset-dir", type=Path, required=True)
    manifest.add_argument("--repository", required=True)
    manifest.add_argument("--output", type=Path, required=True)
    render = subparsers.add_parser("render-nightly")
    render.add_argument("--manifest", type=Path, required=True)
    render.add_argument("--body", type=Path, required=True)
    return result


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        root = ROOT
        if args.command == "validate":
            registry = load_registry(args.registry, root=root)
            print(f"validated {len(registry['components'])} release-channel components")
        elif args.command == "derive":
            component = component_descriptor(args.component, args.registry, root=root)
            base = (
                repository_path(root, component["versionAuthorityFile"])
                .read_text(encoding="utf-8")
                .strip()
            )
            version = derive_nightly_version(base, args.date, args.run)
            print(
                json.dumps({"version": version, "tag": format_tag(component, "nightly", version)})
            )
        elif args.command == "parse-tag":
            component = component_descriptor(args.component, args.registry, root=root)
            print(parse_tag(component, args.channel, args.tag))
        elif args.command == "apply-version":
            for changed in apply_version(
                args.component, args.version, registry_path=args.registry, root=root
            ):
                print(changed)
        elif args.command == "stage-versioned-tree":
            for changed in stage_versioned_tree(
                args.component,
                args.version,
                args.source,
                args.destination,
                registry_path=args.registry,
                root=root,
            ):
                print(changed)
        elif args.command == "plan":
            releases = read_json(args.releases)
            if not isinstance(releases, list):
                raise ChannelError("releases input must be a JSON array")
            result = plan_nightly(
                args.component,
                args.source_sha,
                args.date,
                args.run,
                releases,
                force=args.force,
                registry_path=args.registry,
                root=root,
                attribution_config_path=args.attribution_config,
            )
            rendered = json.dumps(result, indent=2) + "\n"
            if args.output:
                args.output.write_text(rendered, encoding="utf-8")
            else:
                print(rendered, end="")
            if args.github_output:
                write_github_outputs(result, args.github_output)
        elif args.command == "manifest":
            manifest = build_manifest(
                args.component,
                args.version,
                args.tag,
                args.source_sha,
                args.previous_tag,
                args.asset_dir,
                repository=args.repository,
                registry_path=args.registry,
                root=root,
            )
            args.output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        elif args.command == "render-nightly":
            manifest = read_json(args.manifest)
            args.body.write_text(render_nightly_body(manifest), encoding="utf-8")
    except (ChannelError, OSError, ValueError) as error:
        print(f"release channel error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
