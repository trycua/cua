#!/usr/bin/env python3
"""Build and render deterministic, pull-request-first release attribution.

The collector reads the exact local tag range and uses the GitHub API only to
resolve commits to pull requests, contributors, and explicitly closed issues.
It never infers GitHub handles from a git author display name.
"""

from __future__ import annotations

import argparse
import base64
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
CHERRY_PICKED_SOURCE_PR_RE = re.compile(
    r"\bfrom\s*:?[ \t]*(?:(?:https://github\.com/(?P<repository>[^/]+/[^/]+)/pull/)|#)(?P<number>\d+)\b[^\n]{0,200}\bcherry-picked\b",
    re.IGNORECASE,
)
CHERRY_PICKED_FROM_PR_RE = re.compile(
    r"\bcherry-picked\s+from\s*:?[ \t]*(?:(?:https://github\.com/(?P<repository>[^/]+/[^/]+)/pull/)|#)(?P<number>\d+)",
    re.IGNORECASE,
)
VERIFIED_IDENTITY_PR_RE = re.compile(
    r"\bidentity\s+is\s+verified\s+by\b[^\n]{0,80}?(?:(?:https://github\.com/(?P<repository>[^/]+/[^/]+)/pull/)|#)(?P<number>\d+)",
    re.IGNORECASE,
)
TRAILING_PR_RE = re.compile(r"\s+\(#(?P<number>\d+)\)\s*$")
LEGACY_RELEASE_BUMP_RE = re.compile(r"^Bump (?:cua-driver-rs|lume) to v\S+$", re.IGNORECASE)
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

    def pull_commits(self, repository: str, number: int) -> list[dict[str, Any]]:
        commits: list[dict[str, Any]] = []
        for page in range(1, 5):
            batch = list(
                self.get(f"repos/{repository}/pulls/{number}/commits?per_page=100&page={page}")
            )
            commits.extend(batch)
            if len(batch) < 100:
                return commits
        raise ReleaseError(f"pull request #{number} has too many commits to validate safely")

    def file_json(self, repository: str, path: str, ref: str) -> Mapping[str, Any]:
        encoded_path = quote(path.strip("/"), safe="/")
        payload = self.get(f"repos/{repository}/contents/{encoded_path}?ref={quote(ref, safe='')}")
        if not isinstance(payload, Mapping) or payload.get("encoding") != "base64":
            raise ReleaseError(f"GitHub returned an invalid payload for {path} at {ref}")
        try:
            content = base64.b64decode(str(payload.get("content") or ""), validate=False)
            value = json.loads(content.decode("utf-8"))
        except (ValueError, UnicodeDecodeError) as error:
            raise ReleaseError(f"{path} at {ref} is not valid JSON: {error}") from error
        if not isinstance(value, Mapping):
            raise ReleaseError(f"{path} at {ref} must contain a JSON object")
        return value


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
    summary = TRAILING_PR_RE.sub("", match.group("summary").strip()).strip().rstrip(".")
    return ConventionalEntry(
        change_type=match.group("type"),
        scope=match.group("scope"),
        summary=summary,
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


def validate_pr_title(
    title: str,
    *,
    require_release: bool = False,
    allow_non_release: bool = False,
) -> None:
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
    if require_release and entry.change_type not in RELEASING_TYPES and not allow_non_release:
        raise ReleaseError(
            f"pull request type '{entry.change_type}' makes Release Please skip changes to "
            "release-tracked product files; use 'fix(scope): ...' for a patch, "
            "'feat(scope): ...' for a minor release, 'feat(scope)!: ...' for a breaking "
            "release, or add the 'no-release' label when the change is intentionally "
            "non-releasing"
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
    return sorted(
        {
            *_local_reference_numbers(SOURCE_PR_RE, text, repository),
            *_local_reference_numbers(CHERRY_PICKED_SOURCE_PR_RE, text, repository),
            *_local_reference_numbers(CHERRY_PICKED_FROM_PR_RE, text, repository),
            *_local_reference_numbers(VERIFIED_IDENTITY_PR_RE, text, repository),
        }
    )


def _normalized_map(config: Mapping[str, Any], key: str) -> dict[str, str]:
    return {
        str(identity).strip().lower(): str(login).strip()
        for identity, login in dict(config.get(key, {})).items()
    }


def _normalized_set(config: Mapping[str, Any], key: str) -> set[str]:
    return {str(item).strip().lower() for item in config.get(key, [])}


def validate_pr_attribution(
    *,
    repository: str,
    pull: Mapping[str, Any],
    commits: Sequence[Mapping[str, Any]],
    base_config: Mapping[str, Any],
    head_config: Mapping[str, Any],
    github: GitHubClient,
) -> None:
    """Fail closed when preserved human authorship cannot reach a GitHub login.

    The base configuration is trusted policy. The pull request's configuration
    may only satisfy a new identity mapping when an explicitly referenced,
    same-repository source pull request verifies the exact GitHub login.
    """

    bots = _normalized_set(base_config, "bots")
    internal = _normalized_set(base_config, "internalHandles")
    opt_out = _normalized_set(base_config, "optOutHandles")
    ignored = _normalized_set(base_config, "ignoredCoauthorEmails")
    base_overrides = _normalized_map(base_config, "identityOverrides")
    head_overrides = _normalized_map(head_config, "identityOverrides")
    coauthor_overrides = _normalized_map(base_config, "coauthorOverrides")

    changed_existing = {
        email: (login, head_overrides.get(email))
        for email, login in base_overrides.items()
        if head_overrides.get(email) != login
    }
    if changed_existing:
        details = ", ".join(
            f"{email}={actual!r} (expected {expected!r})"
            for email, (expected, actual) in sorted(changed_existing.items())
        )
        raise ReleaseError(
            "the pull request removes or changes trusted identityOverrides: " + details
        )

    pull_number = int(pull.get("number") or 0)
    pull_login = str((pull.get("user") or {}).get("login") or "").strip()
    pull_body_sources = source_pull_numbers(str(pull.get("body") or ""), repository)
    all_pr_sources = set(pull_body_sources)
    unresolved: dict[str, dict[str, Any]] = {}
    preserved_authors: list[dict[str, Any]] = []
    early_errors: list[str] = []

    def resolved_login(name: str, email: str, linked_login: str = "") -> str | None:
        login = (
            linked_login
            or coauthor_overrides.get(f"{name.strip()} <{email}>".lower())
            or login_from_email(email, base_overrides)
        )
        return login or None

    def record_unresolved(
        *, email: str, name: str, sha: str, kind: str, references: Sequence[int]
    ) -> None:
        normalized_email = email.strip().lower()
        if not normalized_email:
            raise ReleaseError(f"commit {sha} has a {kind} with no email address")
        item = unresolved.setdefault(
            normalized_email,
            {"names": set(), "shas": set(), "kinds": set(), "references": set()},
        )
        item["names"].add(name.strip() or "unknown")
        item["shas"].add(sha)
        item["kinds"].add(kind)
        item["references"].update(references)

    for item in commits:
        sha = str(item.get("sha") or "unknown")
        commit = item.get("commit") or {}
        message = str(commit.get("message") or "")
        commit_sources = source_pull_numbers(message, repository)
        all_pr_sources.update(commit_sources)
        references = commit_sources or pull_body_sources

        author = commit.get("author") or {}
        author_name = str(author.get("name") or "")
        author_email = str(author.get("email") or "").strip().lower()
        linked_author = str((item.get("author") or {}).get("login") or "").strip()
        author_login = resolved_login(author_name, author_email, linked_author)
        committer = commit.get("committer") or {}
        committer_email = str(committer.get("email") or "").strip().lower()
        linked_committer = str((item.get("committer") or {}).get("login") or "").strip()
        preserved_unlinked_author = (
            not author_login
            and bool(pull_login)
            and linked_committer.lower() == pull_login.lower()
            and author_email != committer_email
        )
        # An ordinary direct PR author is credited from pull-request metadata even
        # when their commit email is private. A distinct unlinked author committed
        # by the landing PR author is a strong cherry-pick/preservation signal.
        if preserved_unlinked_author and author_email not in ignored:
            record_unresolved(
                email=author_email,
                name=author_name,
                sha=sha,
                kind="commit author",
                references=references,
            )
        elif author_login and (
            author_login.lower() != pull_login.lower()
            and not is_bot(author_login, bots)
            and author_login.lower() not in internal
            and author_login.lower() not in opt_out
        ):
            if not references:
                early_errors.append(
                    f"commit {sha} is authored by @{author_login}, distinct from landing "
                    f"PR author @{pull_login or 'unknown'}, but has no explicit source PR; "
                    "add `Salvaged from #<pr>` so release attribution preserves the author"
                )
            else:
                preserved_authors.append(
                    {
                        "login": author_login,
                        "sha": sha,
                        "references": set(references),
                    }
                )

        for match in COAUTHOR_RE.finditer(message):
            name = match.group("name").strip()
            email = match.group("email").strip().lower()
            login = resolved_login(name, email)
            if email in ignored:
                continue
            if login and (
                is_bot(login, bots)
                or login.lower() in internal
                or login.lower() in opt_out
                or login.lower() == pull_login.lower()
            ):
                continue
            if not login:
                record_unresolved(
                    email=email,
                    name=name,
                    sha=sha,
                    kind="coauthor",
                    references=references,
                )

    new_overrides = {
        email: login for email, login in head_overrides.items() if email not in base_overrides
    }
    if not unresolved and not new_overrides and not preserved_authors and not early_errors:
        return

    source_evidence: dict[int, tuple[str, set[str]]] = {}
    all_references = sorted(
        {
            *all_pr_sources,
            *{number for item in unresolved.values() for number in item["references"]},
        }
    )
    for number in all_references:
        if number == pull_number:
            raise ReleaseError(f"source pull request #{number} is the landing pull request itself")
        source = github.pull(repository, number)
        if int(source.get("number") or 0) != number:
            raise ReleaseError(f"source pull request #{number} could not be verified")
        login = str((source.get("user") or {}).get("login") or "").strip()
        if not login:
            raise ReleaseError(f"source pull request #{number} has no GitHub author")
        source_emails = {
            str(((item.get("commit") or {}).get("author") or {}).get("email") or "").strip().lower()
            for item in github.pull_commits(repository, number)
        }
        source_evidence[number] = (login, source_emails - {""})

    suggestions: dict[str, str] = {}
    errors: list[str] = list(early_errors)
    identities = {
        email: {
            "references": set(identity["references"]),
            "location": ", ".join(sorted(identity["shas"])),
            "description": "/".join(sorted(identity["kinds"])),
        }
        for email, identity in unresolved.items()
    }
    for email in new_overrides:
        identities.setdefault(
            email,
            {
                "references": set(all_pr_sources),
                "location": "the identityOverrides change",
                "description": "new identity override",
            },
        )

    for email, identity in sorted(identities.items()):
        references = sorted(identity["references"])
        if not references:
            errors.append(
                f"{email} ({identity['description']}; {identity['location']}) is "
                "unresolved and has no explicit same-repository source PR; use a linked "
                "GitHub/noreply email or add `Salvaged from #<pr>`"
            )
            continue
        verified = {
            source_evidence[number][0]
            for number in references
            if email in source_evidence[number][1]
        }
        if not verified:
            refs = ", ".join(f"#{number}" for number in references)
            errors.append(
                f"{email} is not an exact commit-author email in the referenced source "
                f"pull request(s) {refs}; use verifiable provenance instead of guessing"
            )
            continue
        logins = verified
        if len(logins) != 1:
            refs = ", ".join(
                f"#{number}=@{source_evidence[number][0]}"
                for number in references
                if email in source_evidence[number][1]
            )
            errors.append(
                f"{email} has ambiguous source-PR authors ({refs}); reference exactly one "
                "contributor identity"
            )
            continue
        suggestions[email] = next(iter(logins))

    for author in preserved_authors:
        referenced_logins = {source_evidence[number][0] for number in author["references"]}
        if author["login"] not in referenced_logins:
            refs = ", ".join(
                f"#{number}=@{source_evidence[number][0]}"
                for number in sorted(author["references"])
            )
            errors.append(
                f"commit {author['sha']} is authored by @{author['login']}, but its "
                f"source reference(s) identify {refs}; reference that author's source PR"
            )

    for email, expected in sorted(suggestions.items()):
        actual = head_overrides.get(email)
        if actual != expected:
            fragment = json.dumps(
                {"identityOverrides": {email: expected}}, indent=2, sort_keys=True
            )
            if actual:
                errors.append(
                    f"identityOverrides maps {email} to @{actual}, but the verified source "
                    f"PR author is @{expected}; use this exact JSON:\n{fragment}"
                )
            else:
                errors.append(
                    f"verified source PR maps {email} uniquely to @{expected}; add this exact "
                    f"JSON to .github/release-attribution-config.json in this PR:\n{fragment}"
                )

    if errors:
        raise ReleaseError("contributor attribution is not merge-ready:\n- " + "\n- ".join(errors))


def select_pull(pulls: Sequence[Mapping[str, Any]], commit_sha: str) -> Mapping[str, Any]:
    if not pulls:
        raise ReleaseError(f"commit {commit_sha} is not associated with a pull request")
    exact = [pull for pull in pulls if pull.get("merge_commit_sha") == commit_sha]
    candidates = exact or [pull for pull in pulls if pull.get("merged_at")]
    if not candidates:
        candidates = list(pulls)
    candidates = sorted(candidates, key=lambda pull: int(pull.get("number", 0)))
    return candidates[-1]


def resolve_pull_for_commit(
    github: GitHubClient,
    repository: str,
    commit: CommitRecord,
    *,
    required: bool = True,
) -> Mapping[str, Any] | None:
    """Resolve a commit to its merged PR, including an exact squash-title fallback."""
    pulls = github.pulls_for_commit(repository, commit.sha)
    if pulls:
        selected = select_pull(pulls, commit.sha)
        return github.pull(repository, int(selected["number"]))

    reference = TRAILING_PR_RE.search(commit.subject)
    if not reference:
        if not required:
            return None
        raise ReleaseError(f"commit {commit.sha} is not associated with a pull request")

    pull = github.pull(repository, int(reference.group("number")))
    expected_title = TRAILING_PR_RE.sub("", commit.subject).strip()
    if not pull.get("merged_at") or str(pull.get("title") or "").strip() != expected_title:
        raise ReleaseError(
            f"commit {commit.sha} has an unverified pull request suffix "
            f"#{reference.group('number')}"
        )
    return pull


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


def changelog_references_change(
    changelog_section: str,
    pull_number: int,
    commit_shas: Iterable[str],
) -> bool:
    """Accept Release Please's PR link or its verified commit-link fallback."""
    if f"#{pull_number}" in changelog_section or f"/pull/{pull_number}" in changelog_section:
        return True
    return any(commit_sha in changelog_section for commit_sha in commit_shas)


def release_bump(changes: Sequence[Mapping[str, Any]], version: str) -> str:
    """Classify a release using the same pre-major policy as Release Please."""
    major_match = re.match(r"^(?P<major>0|[1-9]\d*)\.", version)
    if not major_match:
        raise ReleaseError(f"release version is not semantic: {version}")
    if any(change["breaking"] for change in changes):
        return "minor" if int(major_match.group("major")) == 0 else "major"
    if any(change["type"] == "feat" for change in changes):
        return "minor"
    return "patch"


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
    pull_commits: dict[int, set[str]] = defaultdict(set)
    all_contributors: list[dict[str, Any]] = []
    visual_requested = False

    for commit in commits_in_range(repo_root, previous_tag, current_ref, paths, exclude_paths):
        if re.match(
            r"^chore(?:\([^)]+\))?: release\b", commit.subject, re.IGNORECASE
        ) or LEGACY_RELEASE_BUMP_RE.match(commit.subject):
            continue
        parsed_subject = parse_conventional_line(commit.subject)
        subject_is_releasing = bool(
            parsed_subject and parsed_subject.change_type in RELEASING_TYPES
        )
        pull = resolve_pull_for_commit(
            github,
            repository,
            commit,
            required=subject_is_releasing,
        )
        if pull is None:
            continue
        pull_body = str(pull.get("body") or "")
        if not subject_is_releasing and not OVERRIDE_RE.search(pull_body):
            continue
        pull_number = int(pull["number"])
        pull_commits[pull_number].add(commit.sha)
        entries = release_entries(commit.subject, commit.body, pull_body)
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
            if not changelog_references_change(
                changelog_section,
                int(change["pr"]),
                pull_commits[int(change["pr"])],
            )
        }
    )
    if missing_prs:
        raise ReleaseError(f"changelog section is missing pull requests: {missing_prs}")

    bump = release_bump(changes, version)

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


def validate_pr_command(args: argparse.Namespace) -> None:
    event = json.loads(args.event.read_text())
    event_pull = event.get("pull_request") or {}
    repository = str((event.get("repository") or {}).get("full_name") or "")
    number = int(event_pull.get("number") or event.get("number") or 0)
    if not repository or not number:
        raise ReleaseError("event does not identify a repository and pull request")

    client = GitHubClient(
        os.environ.get("GH_TOKEN", ""), os.environ.get("GITHUB_API_URL", "https://api.github.com")
    )
    pull = client.pull(repository, number)
    head = pull.get("head") or {}
    head_repository = str((head.get("repo") or {}).get("full_name") or "")
    head_sha = str(head.get("sha") or "")
    if not head_repository or not head_sha:
        raise ReleaseError(f"pull request #{number} has no readable head repository")

    commits = client.pull_commits(repository, number)
    expected_commits = int(pull.get("commits") or len(commits))
    if len(commits) != expected_commits:
        raise ReleaseError(
            f"GitHub returned {len(commits)} of {expected_commits} commits for pull request "
            f"#{number}; refusing partial attribution validation"
        )

    base_config = json.loads(args.config.read_text())
    head_config = client.file_json(head_repository, args.config.as_posix(), head_sha)
    validate_pr_attribution(
        repository=repository,
        pull=pull,
        commits=commits,
        base_config=base_config,
        head_config=head_config,
        github=client,
    )
    print(f"contributor attribution is merge-ready for pull request #{number}")


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
    validate.add_argument("--require-release", action="store_true")
    validate.add_argument("--allow-non-release", action="store_true")

    validate_pr = subparsers.add_parser("validate-pr")
    validate_pr.add_argument("--event", type=Path, required=True)
    validate_pr.add_argument(
        "--config", type=Path, default=Path(".github/release-attribution-config.json")
    )

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
            validate_pr_title(
                args.title,
                require_release=args.require_release,
                allow_non_release=args.allow_non_release,
            )
            print("release title is valid")
        elif args.command == "validate-pr":
            validate_pr_command(args)
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
