#!/usr/bin/env python3
"""Give a Release Please component's GitHub release the Cua Driver/Lume notes.

Cua Driver and Lume render their release body in their own CD release job:
`release_attribution.py collect` builds a PR-first manifest of the tag range,
and `render` turns it into the body with Summary, Features/Fixes, a
Contributors section that @-mentions every external contributor (GitHub draws
the release's contributor avatars from those mentions), and the full changelog
link. Every other Release Please component gets the same body from
`.github/workflows/release-notes.yml` through this script:

    release_notes.py plan --tag cua-sdk-v0.4.2
    release_notes.py render --tag cua-sdk-v0.4.2 --sha <sha> --repository trycua/cua --out-dir out
    release_notes.py apply --tag cua-sdk-v0.4.2 --repository trycua/cua --body out/release-body.md

`publish-draft` lets a legacy CD workflow publish the Release Please draft it
built assets for, instead of creating a second release for the same tag.

The component, its paths, exclusions and changelog all come from
`release-please-config.json`, so the notes cover exactly the commits Release
Please put in the changelog. Prerelease tags (a Spaces beta) are not managed
here: they keep the notes their CD workflow writes.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import sys
import time
from typing import Any, Mapping, Sequence

sys.path.insert(0, str(Path(__file__).resolve().parent))

import release_attribution  # noqa: E402
from github_release import GitHubApi, ReleaseError, releases_by_tag, upload_assets  # noqa: E402


STABLE_VERSION_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
# These render and publish their body inside their own CD release job.
SELF_RENDERED = {"cua-driver-rs", "lume", "cua-perception"}
DISPLAY_NAMES = {
    "cua-spaces": "Cua Spaces",
    "cua-sdk": "cua SDK",
    "cua-spacesd": "cua-spacesd",
    "sandbox": "Cua Sandbox",
    "agent": "Cua Agent",
    "core": "Cua Core",
    "bench": "Cua Bench",
    "bench-ui": "Cua Bench UI",
    "som": "Cua SOM",
    "sandbox-apps": "Cua Sandbox Apps",
    "train": "Cua Train",
    "npm-core": "@trycua/core",
    "npm-playground": "@trycua/playground",
    "cuabot": "CuaBot",
}
# component -> (registry, package); the footer names the exact published version.
INSTALL = {
    "cua-sdk": (("pypi", "cua"), ("npm", "@trycua/cua")),
    "sandbox": (("pypi", "cua-sandbox"),),
    "agent": (("pypi", "cua-agent"),),
    "core": (("pypi", "cua-core"),),
    "bench": (("pypi", "cua-bench"),),
    "bench-ui": (("pypi", "cua-bench-ui"),),
    "som": (("pypi", "cua-som"),),
    "sandbox-apps": (("pypi", "cua-sandbox-apps"),),
    "train": (("pypi", "cua-train"),),
    "npm-core": (("npm", "@trycua/core"),),
    "npm-playground": (("npm", "@trycua/playground"),),
    "cuabot": (("npm", "cuabot"),),
}


# The last bump2version tag of each package that moved to Release Please. The
# first Release Please changelog after one of them can miss changes: Release
# Please only searches the newest 500 merges and loses the file list of a
# commit over GitHub's 3,000-file limit (the launch merge #4397). For that one
# release the notes come from git alone, so they still list every change; the
# check that the changelog cites each pull request starts with the next one.
LEGACY_BASELINE_TAGS = frozenset({
    "agent-v0.9.0",
    "core-v0.3.2",
    "bench-v0.3.0",
    "bench-ui-v0.7.1",
    "som-v0.1.4",
    "sandbox-apps-v0.1.2",
    "train-v0.1.2",
    "npm-core-v0.1.6",
    "npm-playground-v0.3.0",
    "cuabot-v1.0.16",
})


def load_config(repo_root: Path) -> Mapping[str, Any]:
    return json.loads((repo_root / "release-please-config.json").read_text())


def resolve_tag(config: Mapping[str, Any], tag: str) -> dict[str, Any] | None:
    """The Release Please package a stable component tag belongs to, if any."""
    if config.get("include-component-in-tag") is not True or config.get("tag-separator") != "-":
        raise ReleaseError("release-please-config.json must tag as <component>-v<version>")
    matches = []
    for path, package in config["packages"].items():
        component = str(package["component"])
        prefix = f"{component}-v"
        if not tag.startswith(prefix):
            continue
        version = tag[len(prefix):]
        if STABLE_VERSION_RE.fullmatch(version):
            matches.append((path, package, component, prefix, version))
    if not matches:
        return None
    if len(matches) > 1:
        raise ReleaseError(f"tag {tag} matches several components: {[m[2] for m in matches]}")
    path, package, component, prefix, version = matches[0]
    # Release Please gives each changed file to the most specific package path,
    # so a package nested under this one owns its own commits.
    nested = [other for other in config["packages"] if other != path and other.startswith(f"{path}/")]
    return {
        "component": component,
        "display_name": DISPLAY_NAMES.get(component, component),
        "version": version,
        "tag": tag,
        "tag_prefix": prefix,
        # The pinned release-please 17.3.0 has no `additional-paths` support
        # (the key is ignored), so the changelog covers the package path only.
        "paths": [path],
        "exclude_paths": [*package.get("exclude-paths", []), *nested],
        "changelog": f"{path}/{package.get('changelog-path', 'CHANGELOG.md')}",
        "self_rendered": component in SELF_RENDERED,
    }


def install_footer(component: str, version: str) -> str:
    lines = []
    for registry, name in INSTALL.get(component, ()):
        if registry == "pypi":
            lines.append(f'pip install "{name}=={version}"')
        else:
            lines.append(f"npm install {name}@{version}")
    if not lines:
        return ""
    return "## Install\n\n```bash\n" + "\n".join(lines) + "\n```\n"


def render(
    *,
    repo_root: Path,
    repository: str,
    tag: str,
    sha: str,
    out_dir: Path,
    ref: str | None,
    github: release_attribution.GitHubClient | None = None,
) -> Path:
    plan = resolve_tag(load_config(repo_root), tag)
    if plan is None or plan["self_rendered"]:
        raise ReleaseError(f"{tag} is not a stable tag of a component this script renders")
    attribution = json.loads((repo_root / ".github/release-attribution-config.json").read_text())
    previous = release_attribution.find_previous_tag(repo_root, tag, plan["tag_prefix"])
    client = github or release_attribution.GitHubClient(
        os.environ.get("GH_TOKEN", ""), os.environ.get("GITHUB_API_URL", "https://api.github.com")
    )
    cites_every_change = release_attribution.changelog_references_change
    if previous in LEGACY_BASELINE_TAGS:
        print(f"{previous} is a legacy baseline: not requiring the changelog to cite every change")
        release_attribution.changelog_references_change = lambda *_args: True
    try:
        manifest = _build(plan, repo_root, repository, tag, previous, sha, attribution, client, ref)
    finally:
        release_attribution.changelog_references_change = cites_every_change
    body = release_attribution.render_body(manifest, install_footer(plan["component"], plan["version"]))
    if len(body.encode()) > 125_000:
        raise ReleaseError("rendered GitHub release body exceeds 125,000 bytes")
    out_dir.mkdir(parents=True, exist_ok=True)
    release_attribution.write_json(out_dir / "release-manifest.json", manifest)
    body_path = out_dir / "release-body.md"
    body_path.write_text(body)
    print(f"rendered {tag}: {len(manifest['changes'])} changes, "
          f"{sum(1 for c in manifest['contributors'] if c.get('external'))} external contributors")
    return body_path


def _build(plan, repo_root, repository, tag, previous, sha, attribution, client, ref):
    return release_attribution.build_manifest(
        repo_root=repo_root,
        repository=repository,
        product=plan["component"],
        display_name=plan["display_name"],
        version=plan["version"],
        tag=tag,
        previous_tag=previous,
        expected_sha=sha,
        paths=plan["paths"],
        changelog_path=(repo_root / plan["changelog"]).resolve(),
        attribution_config=attribution,
        github=client,
        release_ref=ref,
        exclude_paths=plan["exclude_paths"],
    )


def wait_for_release(api: Any, repository: str, tag: str, attempts: int, delay: float) -> dict[str, Any]:
    """Release Please creates the draft and its tag together; allow a short lag."""
    for attempt in range(1, attempts + 1):
        matches = releases_by_tag(api, repository, tag)
        if len(matches) > 1:
            raise ReleaseError(f"expected one GitHub release for {tag}, found {[m.get('id') for m in matches]}")
        if matches:
            return matches[0]
        if attempt < attempts:
            time.sleep(delay)
    raise ReleaseError(f"no GitHub release exists for {tag}")


def apply(api: Any, repository: str, tag: str, body: str, *, attempts: int = 20, delay: float = 15) -> bool:
    """Replace only the release body; draft, prerelease and assets stay as they are."""
    release = wait_for_release(api, repository, tag, attempts, delay)
    if str(release.get("body") or "") == body:
        print(f"{tag} already has the rendered notes")
        return False
    api.patch(f"repos/{repository}/releases/{release['id']}", {"body": body})
    print(f"updated the notes of {tag} ({'draft' if release.get('draft') else 'published'})")
    return True


def publish_draft(
    api: Any,
    repository: str,
    tag: str,
    *,
    asset_dir: Path | None,
    prerelease: bool,
    name: str | None = None,
) -> bool:
    """Publish the Release Please draft for `tag`; False when there is none.

    Keeps the draft's body (Release Please's changelog, or the rendered notes)
    and never makes a monorepo component the repository-wide Latest release.
    """
    matches = releases_by_tag(api, repository, tag)
    if not matches:
        return False
    if len(matches) > 1:
        raise ReleaseError(f"expected one GitHub release for {tag}, found {[m.get('id') for m in matches]}")
    release = matches[0]
    if asset_dir is not None and asset_dir.is_dir() and any(p.is_file() for p in asset_dir.iterdir()):
        upload_assets(api, repository, release, asset_dir)
    if release.get("draft"):
        fields: dict[str, Any] = {"draft": False, "prerelease": prerelease, "make_latest": "false"}
        if name:
            fields["name"] = name
        api.patch(f"repos/{repository}/releases/{release['id']}", fields)
        print(f"published the Release Please draft for {tag}")
    else:
        print(f"{tag} is already published")
    return True


def write_output(name: str, value: str) -> None:
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a") as handle:
            handle.write(f"{name}={value}\n")
    print(f"{name}={value}")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    sub = parser.add_subparsers(dest="command", required=True)
    plan_cmd = sub.add_parser("plan")
    plan_cmd.add_argument("--tag", required=True)
    render_cmd = sub.add_parser("render")
    render_cmd.add_argument("--tag", required=True)
    render_cmd.add_argument("--sha", required=True)
    render_cmd.add_argument("--repository", required=True)
    render_cmd.add_argument("--out-dir", type=Path, required=True)
    render_cmd.add_argument("--ref", help="git ref to read before the tag exists (a release PR head)")
    apply_cmd = sub.add_parser("apply")
    apply_cmd.add_argument("--tag", required=True)
    apply_cmd.add_argument("--repository", required=True)
    apply_cmd.add_argument("--body", type=Path, required=True)
    publish_cmd = sub.add_parser("publish-draft")
    publish_cmd.add_argument("--tag", required=True)
    publish_cmd.add_argument("--repository", required=True)
    publish_cmd.add_argument("--asset-dir", type=Path)
    publish_cmd.add_argument("--prerelease", action="store_true")
    publish_cmd.add_argument("--name", help="release title (the legacy CD workflow's release_name)")
    args = parser.parse_args(argv)
    repo_root = args.repo_root.resolve()
    try:
        if args.command == "plan":
            plan = resolve_tag(load_config(repo_root), args.tag)
            managed = plan is not None and not plan["self_rendered"]
            write_output("render", "true" if managed else "false")
            if managed:
                write_output("component", plan["component"])
                write_output("version", plan["version"])
        elif args.command == "render":
            render(
                repo_root=repo_root,
                repository=args.repository,
                tag=args.tag,
                sha=args.sha,
                out_dir=args.out_dir,
                ref=args.ref,
            )
        elif args.command == "apply":
            api = GitHubApi(os.environ.get("GH_TOKEN", ""), os.environ.get("GITHUB_API_URL", "https://api.github.com"))
            apply(api, args.repository, args.tag, args.body.read_text())
        else:
            api = GitHubApi(os.environ.get("GH_TOKEN", ""), os.environ.get("GITHUB_API_URL", "https://api.github.com"))
            adopted = publish_draft(
                api,
                args.repository,
                args.tag,
                asset_dir=args.asset_dir,
                prerelease=args.prerelease,
                name=args.name,
            )
            write_output("adopted", "true" if adopted else "false")
    except (ReleaseError, release_attribution.ReleaseError) as error:
        print(f"release notes error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
