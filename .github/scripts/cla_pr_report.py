#!/usr/bin/env python3
"""Post the CLA status and "please sign" comment for one pull request.

Runs only from privileged workflows that never check out pull request code:
cla-report.yml (workflow_run, number taken from an untrusted artifact) and the
issue_comment job in cla.yml. Everything about the pull request is read from the
GitHub API; the number is only an identifier and is validated as an integer.

Signatures live in signatures/version1/cla.json on the `cla-signatures` branch. The
`cla` commit status is informational (the required `contributor-attribution` check
enforces the CLA); the comment tells community authors how to sign.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path
from typing import Any, Mapping

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_attribution as ra  # noqa: E402

MARKER = "<!-- cla-report -->"
SIGN_PHRASE = "I have read the CLA Document and I hereby sign the CLA"
STATUS_CONTEXT = "cla"
NUMBER_RE = re.compile(r"[1-9][0-9]{0,8}")


def parse_number(raw: str) -> int:
    raw = raw.strip()
    if not NUMBER_RE.fullmatch(raw):
        raise SystemExit(f"not a pull request number: {raw[:40]!r}")
    return int(raw)


def is_exempt(pull: Mapping[str, Any]) -> bool:
    user = pull.get("user") or {}
    return pull.get("author_association") in ("MEMBER", "OWNER") or user.get("type") == "Bot"


def unsigned_for(client: Any, repository: str, pull: Mapping[str, Any], config_path: Path) -> list[str]:
    entries = client.file_json(repository, ra.CLA_SIGNATURES_PATH, ra.CLA_SIGNATURES_BRANCH).get(
        "signedContributors"
    )
    if not isinstance(entries, list):
        raise ra.ReleaseError("signatures file has no signedContributors list")
    names = {str(e.get("name") or "").lower() for e in entries if isinstance(e, Mapping)}
    match = ra.CLA_ALLOWLIST_RE.search(ra.CLA_WORKFLOW.read_text())
    if not match:
        raise ra.ReleaseError(".github/workflows/cla.yml has no readable allowlist")
    allowed = {login.strip().lower() for login in match.group("logins").split(",")}
    commits = client.all_pull_commits(repository, pull)
    return ra.unsigned_cla_contributors(
        repository=repository,
        pull=pull,
        commits=commits,
        config=json.loads(config_path.read_text()),
        signed=names | allowed,
        github=client,
    )


def comment_body(problems: list[str], document_url: str) -> str:
    lines = "\n".join(f"- {p}" for p in problems)
    return (
        f"{MARKER}\n"
        "Thank you for your contribution. Like many open-source projects, we ask contributors "
        f"to sign our [Contributor License Agreement]({document_url}) before we can accept a "
        "change.\n\n"
        f"{lines}\n\n"
        "To sign, post a comment on this pull request with exactly this text:\n\n"
        f"```\n{SIGN_PHRASE}\n```\n\n"
        "Comment `recheck` to run the check again.\n"
    )


def marker_comments(client: Any, repository: str, number: int) -> list[Mapping[str, Any]]:
    found: list[Mapping[str, Any]] = []
    page = 1
    while True:
        batch = client.get(f"repos/{repository}/issues/{number}/comments?per_page=100&page={page}")
        found += [c for c in batch if MARKER in str(c.get("body") or "")]
        if len(batch) < 100:
            return found
        page += 1


def run(client: Any, repository: str, number: int, head_sha: str, config_path: Path) -> int:
    pull = client.pull(repository, number)
    current = str((pull.get("head") or {}).get("sha") or "")
    if head_sha and current != head_sha:
        print(f"head moved ({head_sha[:10]} -> {current[:10]}); a newer run reports it")
        return 0
    if pull.get("state") != "open":
        print("pull request is not open; nothing to report")
        return 0
    base = f"repos/{repository}"
    existing = marker_comments(client, repository, number)
    if is_exempt(pull):
        problems: list[str] = []
        description = "Org member or bot: no CLA needed"
    else:
        try:
            problems = unsigned_for(client, repository, pull, config_path)
        except (ra.ReleaseError, OSError, ValueError) as error:
            # Cannot verify: never block, never comment. contributor-attribution is the gate.
            print(f"::warning::could not verify CLA signatures: {error}")
            return 0
        description = "All contributors have signed the CLA" if not problems else "CLA not signed"
    client.send(
        "POST",
        f"{base}/statuses/{current}",
        {
            "state": "success" if not problems else "failure",
            "context": STATUS_CONTEXT,
            "description": description[:140],
        },
    )
    if problems:
        body = comment_body(problems, f"https://github.com/{repository}/blob/main/legal/CLA.md")
        if existing:
            if existing[0].get("body") != body:
                client.send("PATCH", f"{base}/issues/comments/{existing[0]['id']}", {"body": body})
        else:
            client.send("POST", f"{base}/issues/{number}/comments", {"body": body})
    else:
        for comment in existing:
            try:
                client.send("DELETE", f"{base}/issues/comments/{comment['id']}", {})
            except ValueError:
                pass  # 204 No Content has no JSON body
    print(description)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY", ""))
    parser.add_argument("--number", required=True, help="pull request number (validated)")
    parser.add_argument("--head-sha", default="", help="skip when the pull request head moved")
    parser.add_argument("--config", type=Path, default=Path(".github/release-attribution-config.json"))
    args = parser.parse_args(argv)
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repository):
        raise SystemExit("bad repository")
    client = ra.GitHubClient(
        os.environ.get("GH_TOKEN", ""), os.environ.get("GITHUB_API_URL", "https://api.github.com")
    )
    return run(client, args.repository, parse_number(args.number), args.head_sha, args.config)


if __name__ == "__main__":
    raise SystemExit(main())
