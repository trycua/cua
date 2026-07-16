#!/usr/bin/env python3
"""Build and render deterministic, pull-request-first release attribution.

The collector reads the exact local tag range and uses the GitHub API only to
resolve commits to pull requests, contributors, and explicitly closed issues.
It never infers GitHub handles from a git author display name.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
from dataclasses import dataclass
from hashlib import sha256
import html
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import textwrap
from typing import Any, Iterable, Mapping, Sequence
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen


CONVENTIONAL_RE = re.compile(
    r"^(?P<type>[a-z][a-z0-9-]*)(?:\((?P<scope>[^)]+)\))?(?P<breaking>!)?:\s+(?P<summary>\S.*)$"
)
COAUTHOR_RE = re.compile(
    r"^Co-authored-by:\s*(?P<name>.*?)\s*<(?P<email>[^>]+)>\s*$",
    re.IGNORECASE | re.MULTILINE,
)
OVERRIDE_RE = re.compile(
    r"BEGIN_COMMIT_OVERRIDE\s*(?P<body>.*?)\s*END_COMMIT_OVERRIDE",
    re.IGNORECASE | re.DOTALL,
)
ISSUE_RE = re.compile(
    r"\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s*:?[ \t]*(?:(?:https://github\.com/(?P<repository>[^/]+/[^/]+)/issues/)|#)(?P<number>\d+)",
    re.IGNORECASE,
)
SOURCE_PR_RE = re.compile(
    r"\b(?:adapted from|based on|salvaged from|source[- ]?pr)\s*:?[ \t]*(?:(?:https://github\.com/(?P<repository>[^/]+/[^/]+)/pull/)|#)(?P<number>\d+)",
    re.IGNORECASE,
)
NOREPLY_RE = re.compile(
    r"^(?:\d+\+)?(?P<login>[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})(?:\[bot\])?)@users\.noreply\.github\.com$",
    re.IGNORECASE,
)
RELEASING_TYPES = {"feat", "fix", "perf", "revert"}
ALLOWED_TITLE_TYPES = RELEASING_TYPES | {
    "build",
    "chore",
    "ci",
    "docs",
    "refactor",
    "style",
    "test",
}
TYPE_HEADINGS = {
    "feat": "Features",
    "fix": "Fixes",
    "perf": "Performance",
    "revert": "Reverts",
}
ROLE_ORDER = {"author": 0, "coauthor": 1, "reporter": 2}
ASSOCIATION_INTERNAL = {"OWNER", "MEMBER", "COLLABORATOR"}


class ReleaseError(RuntimeError):
    """A release invariant failed."""


@dataclass(frozen=True)
class CommitRecord:
    sha: str
    subject: str
    body: str


@dataclass(frozen=True)
class ConventionalEntry:
    change_type: str
    scope: str | None
    summary: str
    breaking: bool


class GitHubClient:
    """Small GitHub REST client with explicit, testable endpoints."""

    def __init__(self, token: str, api_url: str = "https://api.github.com") -> None:
        if not token:
            raise ReleaseError("GH_TOKEN is required to resolve release attribution")
        self.token = token
        self.api_url = api_url.rstrip("/")

    def get(self, path: str) -> Any:
        url = path if path.startswith("http") else f"{self.api_url}/{path.lstrip('/')}"
        request = Request(
            url,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self.token}",
                "User-Agent": "trycua-release-attribution",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        try:
            with urlopen(request, timeout=30) as response:
                return json.load(response)
        except HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")
            raise ReleaseError(f"GitHub API GET {path} failed: {error.code} {detail}") from error

    def pulls_for_commit(self, repository: str, commit_sha: str) -> list[dict[str, Any]]:
        return list(self.get(f"repos/{repository}/commits/{commit_sha}/pulls?per_page=100"))

    def pull(self, repository: str, number: int) -> dict[str, Any]:
        return dict(self.get(f"repos/{repository}/pulls/{number}"))

    def issue(self, repository: str, number: int) -> dict[str, Any]:
        return dict(self.get(f"repos/{repository}/issues/{number}"))


def run_git(repo_root: Path, *args: str) -> str:
    process = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        raise ReleaseError(
            f"git {' '.join(args)} failed ({process.returncode}): {process.stderr.strip()}"
        )
    return process.stdout


def resolve_tag_sha(repo_root: Path, tag: str) -> str:
    return run_git(repo_root, "rev-list", "-n", "1", tag).strip()


def find_previous_tag(repo_root: Path, current_tag: str, tag_prefix: str) -> str | None:
    tags = run_git(repo_root, "tag", "--list", f"{tag_prefix}*", "--sort=-v:refname").splitlines()
    return next((tag for tag in tags if tag and tag != current_tag), None)


def commits_in_range(
    repo_root: Path,
    previous_tag: str | None,
    current_tag: str,
    paths: Sequence[str],
    exclude_paths: Sequence[str] = (),
) -> list[CommitRecord]:
    range_spec = f"{previous_tag}..{current_tag}" if previous_tag else current_tag
    pathspecs = [*paths, *(f":(exclude){path}" for path in exclude_paths)]
    shas = run_git(repo_root, "log", "--reverse", "--format=%H", range_spec, "--", *pathspecs)
    records: list[CommitRecord] = []
    for commit_sha in shas.splitlines():
        if not commit_sha:
            continue
        raw = run_git(repo_root, "show", "-s", "--format=%s%x00%B", commit_sha)
        subject, _, body = raw.partition("\x00")
        records.append(CommitRecord(commit_sha, subject.strip(), body.strip()))
    return records


def parse_conventional_line(line: str) -> ConventionalEntry | None:
    match = CONVENTIONAL_RE.match(line.strip())
    if not match:
        return None
    return ConventionalEntry(
        change_type=match.group("type"),
        scope=match.group("scope"),
        summary=match.group("summary").strip().rstrip("."),
        breaking=bool(match.group("breaking")),
    )


def release_entries(subject: str, commit_body: str, pull_body: str) -> list[ConventionalEntry]:
    override = OVERRIDE_RE.search(pull_body or "")
    candidates = override.group("body").splitlines() if override else [subject]
    entries = [entry for line in candidates if (entry := parse_conventional_line(line))]
    if "BREAKING CHANGE:" in commit_body or "BREAKING-CHANGE:" in commit_body:
        entries = [
            ConventionalEntry(entry.change_type, entry.scope, entry.summary, True)
            for entry in entries
        ]
    return [entry for entry in entries if entry.change_type in RELEASING_TYPES]


def validate_pr_title(title: str) -> None:
    entry = parse_conventional_line(title)
    if not entry:
        raise ReleaseError(
            "pull request title must use Conventional Commit syntax, for example "
            "'fix(driver): preserve input while reconnecting'"
        )
    if entry.change_type not in ALLOWED_TITLE_TYPES:
        allowed = ", ".join(sorted(ALLOWED_TITLE_TYPES))
        raise ReleaseError(
            f"unsupported pull request type '{entry.change_type}'; use one of: {allowed}"
        )


def login_from_email(email: str, overrides: Mapping[str, str]) -> str | None:
    normalized = email.strip().lower()
    if normalized in overrides:
        return overrides[normalized]
    match = NOREPLY_RE.match(normalized)
    return match.group("login") if match else None


def is_bot(login: str, configured_bots: set[str]) -> bool:
    lowered = login.lower()
    return lowered.endswith("[bot]") or lowered in configured_bots


def is_external(association: str | None, login: str, internal_handles: set[str]) -> bool:
    if login.lower() in internal_handles:
        return False
    return (association or "").upper() not in ASSOCIATION_INTERNAL


def _local_reference_numbers(
    pattern: re.Pattern[str], text: str, repository: str | None
) -> list[int]:
    numbers = {
        int(match.group("number"))
        for match in pattern.finditer(text or "")
        if not match.group("repository")
        or not repository
        or match.group("repository").lower() == repository.lower()
    }
    return sorted(numbers)


def linked_issue_numbers(pull_body: str, repository: str | None = None) -> list[int]:
    return _local_reference_numbers(ISSUE_RE, pull_body, repository)


def source_pull_numbers(text: str, repository: str | None = None) -> list[int]:
    return _local_reference_numbers(SOURCE_PR_RE, text, repository)


def select_pull(pulls: Sequence[Mapping[str, Any]], commit_sha: str) -> Mapping[str, Any]:
    if not pulls:
        raise ReleaseError(f"commit {commit_sha} is not associated with a pull request")
    exact = [pull for pull in pulls if pull.get("merge_commit_sha") == commit_sha]
    candidates = exact or [pull for pull in pulls if pull.get("merged_at")]
    if not candidates:
        candidates = list(pulls)
    candidates = sorted(candidates, key=lambda pull: int(pull.get("number", 0)))
    return candidates[-1]


def contributor(
    login: str,
    role: str,
    external: bool,
) -> dict[str, Any]:
    return {"login": login, "role": role, "external": external}


def merge_contributors(items: Iterable[Mapping[str, Any]]) -> list[dict[str, Any]]:
    by_login: dict[str, dict[str, Any]] = {}
    for item in items:
        login = str(item["login"])
        existing = by_login.setdefault(
            login,
            {"login": login, "roles": set(), "external": bool(item.get("external", True))},
        )
        existing["roles"].add(str(item["role"]))
        existing["external"] = existing["external"] and bool(item.get("external", True))
    return [
        {
            "login": item["login"],
            "roles": sorted(item["roles"], key=lambda role: ROLE_ORDER.get(role, 99)),
            "external": item["external"],
        }
        for item in sorted(by_login.values(), key=lambda value: value["login"].lower())
    ]


def asset_checksums(asset_dir: Path | None) -> list[dict[str, Any]]:
    if not asset_dir:
        return []
    assets: list[dict[str, Any]] = []
    for path in sorted(asset_dir.rglob("*")):
        if not path.is_file() or path.name in {
            "release-manifest.json",
            "release-body.md",
            "release-social.txt",
            "release-card.svg",
            "release-card.png",
        }:
            continue
        digest = sha256(path.read_bytes()).hexdigest()
        assets.append(
            {
                "name": path.relative_to(asset_dir).as_posix(),
                "bytes": path.stat().st_size,
                "sha256": digest,
            }
        )
    return assets


def _change_contributors(
    pull: Mapping[str, Any],
    commit: CommitRecord,
    github: GitHubClient,
    repository: str,
    config: Mapping[str, Any],
) -> tuple[list[dict[str, Any]], list[int], bool]:
    bots = {str(item).lower() for item in config.get("bots", [])}
    overrides = {
        str(email).lower(): str(login)
        for email, login in dict(config.get("identityOverrides", {})).items()
    }
    coauthor_overrides = {
        str(identity).lower(): str(login)
        for identity, login in dict(config.get("coauthorOverrides", {})).items()
    }
    ignored_coauthor_emails = {
        str(item).lower() for item in config.get("ignoredCoauthorEmails", [])
    }
    internal = {str(item).lower() for item in config.get("internalHandles", [])}
    opt_out = {str(item).lower() for item in config.get("optOutHandles", [])}
    items: list[dict[str, Any]] = []

    author_login = str((pull.get("user") or {}).get("login") or "")
    if author_login and not is_bot(author_login, bots) and author_login.lower() not in opt_out:
        items.append(
            contributor(
                author_login,
                "author",
                is_external(str(pull.get("author_association") or ""), author_login, internal),
            )
        )

    for match in COAUTHOR_RE.finditer(commit.body):
        email = match.group("email").strip().lower()
        if email in ignored_coauthor_emails:
            continue
        identity = f"{match.group('name').strip()} <{email}>".lower()
        login = coauthor_overrides.get(identity) or login_from_email(email, overrides)
        if not login:
            raise ReleaseError(
                f"commit {commit.sha} has an unresolved human coauthor email; "
                "add an exceptional identityOverrides entry"
            )
        if is_bot(login, bots) or login.lower() in opt_out:
            continue
        items.append(contributor(login, "coauthor", login.lower() not in internal))

    source_numbers = source_pull_numbers(f"{commit.body}\n{pull.get('body') or ''}", repository)
    for number in source_numbers:
        source = github.pull(repository, number)
        login = str((source.get("user") or {}).get("login") or "")
        if not login or is_bot(login, bots) or login.lower() in opt_out:
            continue
        items.append(
            contributor(
                login,
                "coauthor",
                is_external(str(source.get("author_association") or ""), login, internal),
            )
        )

    issues = linked_issue_numbers(str(pull.get("body") or ""), repository)
    for number in issues:
        issue = github.issue(repository, number)
        if issue.get("pull_request"):
            continue
        login = str((issue.get("user") or {}).get("login") or "")
        if not login or is_bot(login, bots) or login.lower() in opt_out:
            continue
        items.append(
            contributor(
                login,
                "reporter",
                is_external(str(issue.get("author_association") or ""), login, internal),
            )
        )

    labels = {str((label or {}).get("name") or "") for label in pull.get("labels", [])}
    return _dedupe_change_contributors(items), issues, "release-visual" in labels


def _dedupe_change_contributors(items: Iterable[Mapping[str, Any]]) -> list[dict[str, Any]]:
    unique: dict[tuple[str, str], dict[str, Any]] = {}
    for item in items:
        key = (str(item["login"]).lower(), str(item["role"]))
        unique[key] = dict(item)
    return sorted(
        unique.values(),
        key=lambda item: (ROLE_ORDER.get(str(item["role"]), 99), str(item["login"]).lower()),
    )


def extract_changelog_section(changelog: Path, version: str) -> str:
    content = changelog.read_text()
    pattern = re.compile(
        rf"^##\s+(?:\[{re.escape(version)}\]|{re.escape(version)})(?:\s|\(|$).*?(?=^##\s|\Z)",
        re.MULTILINE | re.DOTALL,
    )
    match = pattern.search(content)
    if not match:
        raise ReleaseError(f"{changelog} has no changelog section for {version}")
    return match.group(0).rstrip() + "\n"


def build_manifest(
    *,
    repo_root: Path,
    repository: str,
    product: str,
    display_name: str,
    version: str,
    tag: str,
    previous_tag: str | None,
    expected_sha: str,
    paths: Sequence[str],
    changelog_path: Path,
    attribution_config: Mapping[str, Any],
    github: GitHubClient,
    release_ref: str | None = None,
    exclude_paths: Sequence[str] = (),
    asset_dir: Path | None = None,
) -> dict[str, Any]:
    current_ref = release_ref or tag
    actual_sha = resolve_tag_sha(repo_root, current_ref)
    if actual_sha != expected_sha:
        raise ReleaseError(
            f"release ref {current_ref} points to {actual_sha}, expected {expected_sha}"
        )

    changes: list[dict[str, Any]] = []
    seen_changes: set[tuple[int, str, str]] = set()
    all_contributors: list[dict[str, Any]] = []
    visual_requested = False

    for commit in commits_in_range(repo_root, previous_tag, current_ref, paths, exclude_paths):
        if re.match(r"^chore(?:\([^)]+\))?: release\b", commit.subject, re.IGNORECASE):
            continue
        parsed_subject = parse_conventional_line(commit.subject)
        if parsed_subject and parsed_subject.change_type not in RELEASING_TYPES:
            continue

        selected = select_pull(github.pulls_for_commit(repository, commit.sha), commit.sha)
        pull_number = int(selected["number"])
        pull = github.pull(repository, pull_number)
        entries = release_entries(commit.subject, commit.body, str(pull.get("body") or ""))
        if not entries:
            continue
        contributors, issues, pull_visual = _change_contributors(
            pull, commit, github, repository, attribution_config
        )
        visual_requested = visual_requested or pull_visual
        all_contributors.extend(contributors)

        for entry in entries:
            key = (pull_number, entry.change_type, entry.summary.lower())
            if key in seen_changes:
                continue
            seen_changes.add(key)
            changes.append(
                {
                    "type": entry.change_type,
                    "scope": entry.scope,
                    "summary": entry.summary,
                    "breaking": entry.breaking,
                    "pr": pull_number,
                    "issues": issues,
                    "contributors": contributors,
                }
            )

    if not changes:
        raise ReleaseError(f"no releasing pull requests found for {tag}")

    changelog_section = extract_changelog_section(changelog_path, version)
    missing_prs = sorted(
        {
            int(change["pr"])
            for change in changes
            if f"#{change['pr']}" not in changelog_section
            and f"/pull/{change['pr']}" not in changelog_section
        }
    )
    if missing_prs:
        raise ReleaseError(f"changelog section is missing pull requests: {missing_prs}")

    bump = "patch"
    if any(change["breaking"] for change in changes):
        bump = "major"
    elif any(change["type"] == "feat" for change in changes):
        bump = "minor"

    owner, repo = repository.split("/", 1)
    compare_url = (
        f"https://github.com/{owner}/{repo}/compare/{quote(previous_tag)}...{quote(tag)}"
        if previous_tag
        else f"https://github.com/{owner}/{repo}/releases/tag/{quote(tag)}"
    )
    return {
        "schema": (
            f"https://raw.githubusercontent.com/{repository}/{actual_sha}/"
            ".github/release-manifest.schema.json"
        ),
        "schemaVersion": 1,
        "repository": repository,
        "product": product,
        "displayName": display_name,
        "version": version,
        "bump": bump,
        "tag": tag,
        "sha": actual_sha,
        "previousTag": previous_tag,
        "compareUrl": compare_url,
        "changelog": {
            "path": changelog_path.relative_to(repo_root).as_posix(),
            "sha256": sha256(changelog_section.encode()).hexdigest(),
        },
        "visualRequested": visual_requested or bump == "major",
        "changes": changes,
        "contributors": merge_contributors(all_contributors),
        "assets": asset_checksums(asset_dir),
    }


def _thanks(change: Mapping[str, Any]) -> str:
    external = [
        item
        for item in change.get("contributors", [])
        if item.get("external") and item.get("role") in {"author", "coauthor"}
    ]
    reporters = [
        item
        for item in change.get("contributors", [])
        if item.get("external") and item.get("role") == "reporter"
    ]
    parts: list[str] = []
    if external:
        parts.append("Thanks " + ", ".join(f"@{item['login']}" for item in external))
    if reporters:
        parts.append("reported by " + ", ".join(f"@{item['login']}" for item in reporters))
    return "; ".join(parts)


def render_body(manifest: Mapping[str, Any], footer: str = "") -> str:
    display_name = str(manifest["displayName"])
    version = str(manifest["version"])
    changes = list(manifest["changes"])
    repository_url = f"https://github.com/{manifest['repository']}"
    feature_count = sum(change["type"] == "feat" for change in changes)
    fix_count = sum(change["type"] == "fix" for change in changes)
    if feature_count and fix_count:
        summary = f"This release adds {feature_count} features and includes {fix_count} fixes."
    elif feature_count:
        noun = "feature" if feature_count == 1 else "features"
        summary = f"This release adds {feature_count} {noun}."
    elif fix_count:
        noun = "fix" if fix_count == 1 else "fixes"
        summary = f"This release includes {fix_count} {noun}."
    else:
        noun = "change" if len(changes) == 1 else "changes"
        summary = f"This release includes {len(changes)} user-facing {noun}."

    lines = [f"# {display_name} {version}", "", "## Summary", "", summary]
    if manifest.get("visualRequested"):
        card_url = (
            f"https://github.com/{manifest['repository']}/releases/download/"
            f"{quote(str(manifest['tag']), safe='')}/release-card.png"
        )
        lines.extend(
            [
                "",
                f"![{display_name} {version} release highlights]({card_url})",
            ]
        )
    grouped: dict[str, list[Mapping[str, Any]]] = defaultdict(list)
    for change in changes:
        grouped[str(change["type"])].append(change)
    for change_type in ("feat", "fix", "perf", "revert"):
        if not grouped[change_type]:
            continue
        lines.extend(["", f"## {TYPE_HEADINGS[change_type]}", ""])
        for change in grouped[change_type]:
            suffix = _thanks(change)
            bullet = (
                f"- {change['summary']}. ([#{change['pr']}]({repository_url}/pull/{change['pr']}))"
            )
            if suffix:
                bullet += f" {suffix}."
            lines.append(bullet)

    external = [item for item in manifest["contributors"] if item.get("external")]
    lines.extend(["", "## Contributors", ""])
    if external:
        lines.append(
            "Thanks to "
            + ", ".join(f"@{item['login']}" for item in external)
            + " for contributing to this release."
        )
    else:
        lines.append("This release contains maintainer changes only.")

    if footer.strip():
        lines.extend(["", footer.strip()])

    assets = list(manifest.get("assets", []))
    if assets:
        lines.extend(["", "## SHA256 Checksums", "", "```text"])
        lines.extend(f"{asset['sha256']}  {asset['name']}" for asset in assets)
        lines.append("```")

    lines.extend(
        [
            "",
            "## Full changelog",
            "",
            f"[{manifest.get('previousTag') or 'Initial release'}...{manifest['tag']}]({manifest['compareUrl']})",
            "",
        ]
    )
    return "\n".join(lines)


def render_social(manifest: Mapping[str, Any]) -> str:
    first = str(manifest["changes"][0]["summary"]).rstrip(".")
    external = [item for item in manifest["contributors"] if item.get("external")]
    thanks = ""
    if external:
        noun = "contributor" if len(external) == 1 else "contributors"
        thanks = f"\n\nThanks to {len(external)} community {noun}."
    url = f"https://github.com/{manifest['repository']}/releases/tag/{manifest['tag']}"
    prefix = f"{manifest['displayName']} {manifest['version']} is out.\n\n"
    suffix = f"{thanks}\n\nRelease notes: {url}"
    available = max(40, 280 - len(prefix) - len(suffix))
    if len(first) > available:
        first = first[: available - 1].rstrip() + "…"
    return f"{prefix}{first}.{suffix}\n"


def render_card_svg(manifest: Mapping[str, Any]) -> str:
    changes = list(manifest["changes"])[:5]
    lines: list[str] = []
    y = 640
    for change in changes:
        wrapped = textwrap.wrap(str(change["summary"]), width=44)[:2]
        lines.append(f'<circle cx="170" cy="{y - 13}" r="9" fill="#8BE9FD"/>')
        for index, text in enumerate(wrapped):
            lines.append(
                f'<text x="205" y="{y + index * 48}" class="change">{html.escape(text)}</text>'
            )
        y += 65 + 44 * max(0, len(wrapped) - 1)
    contributors = sum(item.get("external", False) for item in manifest["contributors"])
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1600" viewBox="0 0 1600 1600">
<defs><linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop stop-color="#08111f"/><stop offset="1" stop-color="#123a55"/></linearGradient></defs>
<rect width="1600" height="1600" fill="url(#bg)"/><rect x="80" y="80" width="1440" height="1440" rx="52" fill="none" stroke="#8BE9FD" stroke-width="4" opacity="0.65"/>
<style>.eyebrow{{font:600 34px system-ui;letter-spacing:8px;fill:#8BE9FD}}.title{{font:700 118px system-ui;fill:#fff}}.version{{font:500 64px ui-monospace,monospace;fill:#BD93F9}}.change{{font:500 42px system-ui;fill:#F8F8F2}}.meta{{font:500 32px ui-monospace,monospace;fill:#A8B2C1}}</style>
<text x="150" y="220" class="eyebrow">CUA RELEASE</text><text x="150" y="395" class="title">{html.escape(str(manifest["displayName"]))}</text><text x="150" y="495" class="version">v{html.escape(str(manifest["version"]))}</text>
{"".join(lines)}
<text x="150" y="1430" class="meta">{len(manifest["changes"])} changes · {contributors} external contributors · {html.escape(str(manifest["repository"]))}</text></svg>\n"""


def render_card_alt_text(manifest: Mapping[str, Any]) -> str:
    highlights = "; ".join(str(change["summary"]).rstrip(".") for change in manifest["changes"][:5])
    contributors = sum(item.get("external", False) for item in manifest["contributors"])
    noun = "contributor" if contributors == 1 else "contributors"
    return (
        f"{manifest['displayName']} {manifest['version']} release highlights: "
        f"{highlights}. {len(manifest['changes'])} changes and {contributors} "
        f"external {noun}.\n"
    )


def write_json(path: Path, value: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def collect_command(args: argparse.Namespace) -> None:
    repo_root = args.repo_root.resolve()
    config = json.loads(args.config.read_text())
    previous_tag = args.previous_tag or find_previous_tag(repo_root, args.tag, args.tag_prefix)
    client = GitHubClient(
        os.environ.get("GH_TOKEN", ""), os.environ.get("GITHUB_API_URL", "https://api.github.com")
    )
    manifest = build_manifest(
        repo_root=repo_root,
        repository=args.repository,
        product=args.product,
        display_name=args.display_name,
        version=args.version,
        tag=args.tag,
        previous_tag=previous_tag,
        expected_sha=args.sha,
        paths=args.path,
        changelog_path=(repo_root / args.changelog).resolve(),
        attribution_config=config,
        github=client,
        release_ref=args.ref,
        exclude_paths=args.exclude_path,
        asset_dir=args.asset_dir.resolve() if args.asset_dir else None,
    )
    write_json(args.output, manifest)
    print(f"wrote {args.output} with {len(manifest['changes'])} changes")


def render_command(args: argparse.Namespace) -> None:
    manifest = json.loads(args.manifest.read_text())
    if manifest.get("schemaVersion") != 1:
        raise ReleaseError(f"unsupported release manifest schema {manifest.get('schemaVersion')}")
    footer = args.footer.read_text() if args.footer else ""
    body = render_body(manifest, footer)
    if len(body.encode()) > 125_000:
        raise ReleaseError("rendered GitHub release body exceeds 125,000 bytes")
    for output in (args.body, args.social, args.card, args.alt):
        if output:
            output.parent.mkdir(parents=True, exist_ok=True)
    args.body.write_text(body)
    args.social.write_text(render_social(manifest))
    if args.card and manifest.get("visualRequested"):
        args.card.write_text(render_card_svg(manifest))
        if args.alt:
            args.alt.write_text(render_card_alt_text(manifest))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate-title")
    validate.add_argument("--title", required=True)

    collect = subparsers.add_parser("collect")
    collect.add_argument("--repo-root", type=Path, default=Path.cwd())
    collect.add_argument("--repository", required=True)
    collect.add_argument("--product", required=True)
    collect.add_argument("--display-name", required=True)
    collect.add_argument("--version", required=True)
    collect.add_argument("--tag", required=True)
    collect.add_argument("--ref", help="git ref to inspect before the release tag exists")
    collect.add_argument("--tag-prefix", required=True)
    collect.add_argument("--previous-tag")
    collect.add_argument("--sha", required=True)
    collect.add_argument("--path", action="append", required=True)
    collect.add_argument("--exclude-path", action="append", default=[])
    collect.add_argument("--changelog", type=Path, required=True)
    collect.add_argument(
        "--config", type=Path, default=Path(".github/release-attribution-config.json")
    )
    collect.add_argument("--asset-dir", type=Path)
    collect.add_argument("--output", type=Path, required=True)

    render = subparsers.add_parser("render")
    render.add_argument("--manifest", type=Path, required=True)
    render.add_argument("--body", type=Path, required=True)
    render.add_argument("--social", type=Path, required=True)
    render.add_argument("--card", type=Path)
    render.add_argument("--alt", type=Path)
    render.add_argument("--footer", type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "validate-title":
            validate_pr_title(args.title)
            print("release title is valid")
        elif args.command == "collect":
            collect_command(args)
        elif args.command == "render":
            render_command(args)
        else:
            parser.error(f"unknown command {args.command}")
    except (ReleaseError, OSError, ValueError) as error:
        print(f"release attribution error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
