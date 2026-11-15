#!/usr/bin/env python3
"""Close pull requests from forks that change CI workflow or action files.

Runs on a schedule from close-fork-workflow-prs.yml, on the default branch. It never checks out
or executes pull request code: it lists open pull requests and their changed file names through
the GitHub API, and closes the ones that match.

A pull request is closed when all of these hold:
- its head branch is in a fork (or the fork was deleted);
- its author is not an owner, member or collaborator of the repository;
- it has no `allow-workflow-change` label (only maintainers can add labels);
- it adds, edits, deletes or renames a file under .github/workflows/ or .github/actions/.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.request
from typing import Any, Iterable, Mapping

PROTECTED_PREFIXES = (".github/workflows/", ".github/actions/")
TRUSTED_ASSOCIATIONS = {"OWNER", "MEMBER", "COLLABORATOR"}
OVERRIDE_LABEL = "allow-workflow-change"
MARKER = "<!-- close-fork-workflow-prs -->"
COMMENT = (
    f"{MARKER}\n"
    "Thanks for the contribution. This pull request changes files under `.github/workflows/` "
    "or `.github/actions/`, and we don't accept CI changes from forks, so it has been closed "
    "automatically.\n\n"
    "If you need a CI change, please open an issue describing it and a maintainer will make it. "
    "Changes outside those folders are welcome in a new pull request."
)


def is_fork(pull: Mapping[str, Any], repository: str) -> bool:
    head_repo = (pull.get("head") or {}).get("repo")
    return not head_repo or head_repo.get("full_name") != repository


def is_trusted(pull: Mapping[str, Any]) -> bool:
    if pull.get("author_association") in TRUSTED_ASSOCIATIONS:
        return True
    return any(label.get("name") == OVERRIDE_LABEL for label in pull.get("labels") or [])


def touches_ci(files: Iterable[Mapping[str, Any]]) -> list[str]:
    hits = []
    for f in files:
        for name in (f.get("filename"), f.get("previous_filename")):
            if name and name.startswith(PROTECTED_PREFIXES):
                hits.append(name)
    return sorted(set(hits))


class Client:
    def __init__(self, token: str, api: str = "https://api.github.com") -> None:
        self.token, self.api = token, api.rstrip("/")

    def send(self, method: str, path: str, payload: Any = None) -> Any:
        req = urllib.request.Request(
            f"{self.api}/{path.lstrip('/')}",
            method=method,
            data=None if payload is None else json.dumps(payload).encode(),
            headers={
                "Authorization": f"Bearer {self.token}",
                "Accept": "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        with urllib.request.urlopen(req, timeout=30) as resp:
            body = resp.read()
        return json.loads(body) if body else None

    def paged(self, path: str) -> list[Any]:
        out, page = [], 1
        sep = "&" if "?" in path else "?"
        while True:
            batch = self.send("GET", f"{path}{sep}per_page=100&page={page}")
            out.extend(batch)
            if len(batch) < 100:
                return out
            page += 1


def run(client: Any, repository: str, dry_run: bool) -> list[int]:
    closed = []
    for pull in client.paged(f"repos/{repository}/pulls?state=open"):
        if not is_fork(pull, repository) or is_trusted(pull):
            continue
        number = pull["number"]
        hits = touches_ci(client.paged(f"repos/{repository}/pulls/{number}/files"))
        if not hits:
            continue
        print(f"#{number} by {pull['user']['login']}: changes {', '.join(hits)}")
        closed.append(number)
        if dry_run:
            continue
        comments = client.paged(f"repos/{repository}/issues/{number}/comments")
        if not any(MARKER in (c.get("body") or "") for c in comments):
            client.send("POST", f"repos/{repository}/issues/{number}/comments", {"body": COMMENT})
        client.send("PATCH", f"repos/{repository}/pulls/{number}", {"state": "closed"})
    return closed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    if not args.repository:
        parser.error("--repository or GITHUB_REPOSITORY is required")
    closed = run(Client(os.environ["GH_TOKEN"]), args.repository, args.dry_run)
    verb = "would close" if args.dry_run else "closed"
    print(f"{verb} {len(closed)} pull request(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
