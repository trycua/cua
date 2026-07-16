#!/usr/bin/env python3
"""Upload assets to a Release Please draft and publish it exactly once."""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
from pathlib import Path
import sys
from typing import Any, Mapping, Sequence
from urllib.error import HTTPError
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen


class ReleaseError(RuntimeError):
    """A release lifecycle invariant failed."""


class GitHubApi:
    def __init__(self, token: str, api_url: str = "https://api.github.com") -> None:
        if not token:
            raise ReleaseError("GH_TOKEN is required")
        self.token = token
        self.api_url = api_url.rstrip("/")

    def request(
        self,
        method: str,
        path: str,
        *,
        json_body: Mapping[str, Any] | None = None,
        raw_body: bytes | None = None,
        content_type: str | None = None,
    ) -> Any:
        url = path if path.startswith("http") else f"{self.api_url}/{path.lstrip('/')}"
        headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {self.token}",
            "User-Agent": "trycua-release-finalizer",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        body: bytes | None = raw_body
        if json_body is not None:
            body = json.dumps(json_body).encode()
            headers["Content-Type"] = "application/json"
        elif content_type:
            headers["Content-Type"] = content_type
        request = Request(url, data=body, headers=headers, method=method)
        try:
            with urlopen(request, timeout=120) as response:
                if response.status == 204:
                    return None
                return json.load(response)
        except HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")
            raise ReleaseError(
                f"GitHub API {method} {path} failed: {error.code} {detail}"
            ) from error

    def get(self, path: str) -> Any:
        return self.request("GET", path)

    def delete(self, path: str) -> None:
        self.request("DELETE", path)

    def patch(self, path: str, body: Mapping[str, Any]) -> Any:
        return self.request("PATCH", path, json_body=body)

    def upload(self, url: str, path: Path) -> Any:
        content_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        return self.request("POST", url, raw_body=path.read_bytes(), content_type=content_type)


def releases_by_tag(api: GitHubApi, repository: str, tag: str) -> list[dict[str, Any]]:
    matches: list[dict[str, Any]] = []
    for page in range(1, 11):
        releases = list(api.get(f"repos/{repository}/releases?per_page=100&page={page}"))
        matches.extend(release for release in releases if release.get("tag_name") == tag)
        if len(releases) < 100:
            break
    return matches


def unique_release(api: GitHubApi, repository: str, tag: str) -> dict[str, Any]:
    matches = releases_by_tag(api, repository, tag)
    if not matches:
        raise ReleaseError(f"Release Please draft for {tag} does not exist")
    if len(matches) != 1:
        ids = [release.get("id") for release in matches]
        raise ReleaseError(f"expected one GitHub release for {tag}, found {ids}")
    return matches[0]


def tag_commit_sha(api: GitHubApi, repository: str, tag: str) -> str:
    reference = api.get(f"repos/{repository}/git/ref/tags/{quote(tag, safe='')}")
    target = reference["object"]
    for _ in range(5):
        if target["type"] == "commit":
            return str(target["sha"])
        if target["type"] != "tag":
            raise ReleaseError(f"tag {tag} points to unsupported object type {target['type']}")
        annotated = api.get(f"repos/{repository}/git/tags/{target['sha']}")
        target = annotated["object"]
    raise ReleaseError(f"tag {tag} has too many annotation layers")


def release_assets(api: GitHubApi, repository: str, release_id: int) -> list[dict[str, Any]]:
    assets: list[dict[str, Any]] = []
    for page in range(1, 11):
        result = list(
            api.get(f"repos/{repository}/releases/{release_id}/assets?per_page=100&page={page}")
        )
        assets.extend(result)
        if len(result) < 100:
            break
    return assets


def upload_assets(
    api: GitHubApi,
    repository: str,
    release: Mapping[str, Any],
    asset_dir: Path,
) -> None:
    release_id = int(release["id"])
    upload_url = str(release["upload_url"]).split("{", 1)[0]
    existing = release_assets(api, repository, release_id)
    by_name: dict[str, list[dict[str, Any]]] = {}
    for asset in existing:
        by_name.setdefault(str(asset["name"]), []).append(asset)

    files = sorted(path for path in asset_dir.iterdir() if path.is_file())
    if not files:
        raise ReleaseError(f"asset directory {asset_dir} is empty")
    for path in files:
        matches = by_name.get(path.name, [])
        complete = [
            asset
            for asset in matches
            if asset.get("state") == "uploaded"
            and int(asset.get("size", -1)) == path.stat().st_size
        ]
        if len(complete) == 1 and len(matches) == 1:
            print(f"asset already uploaded with matching size: {path.name}")
            continue
        for asset in matches:
            api.delete(f"repos/{repository}/releases/assets/{asset['id']}")
            print(f"removed incomplete or duplicate asset: {path.name} ({asset['id']})")
        query = urlencode({"name": path.name})
        api.upload(f"{upload_url}?{query}", path)
        print(f"uploaded {path.name}")


def verify_published_assets(
    api: GitHubApi,
    repository: str,
    release: Mapping[str, Any],
    asset_dir: Path,
) -> None:
    existing = release_assets(api, repository, int(release["id"]))
    by_name: dict[str, list[dict[str, Any]]] = {}
    for asset in existing:
        by_name.setdefault(str(asset["name"]), []).append(asset)
    files = sorted(path for path in asset_dir.iterdir() if path.is_file())
    if not files:
        raise ReleaseError(f"asset directory {asset_dir} is empty")
    for path in files:
        matches = by_name.get(path.name, [])
        if len(matches) != 1:
            raise ReleaseError(
                f"published release asset {path.name} has {len(matches)} matching uploads"
            )
        asset = matches[0]
        if asset.get("state") != "uploaded" or int(asset.get("size", -1)) != path.stat().st_size:
            raise ReleaseError(f"published release asset {path.name} does not match the local file")


def finalize_release(
    *,
    api: GitHubApi,
    repository: str,
    tag: str,
    expected_sha: str,
    body: str,
    asset_dir: Path,
    prerelease: bool,
    make_latest: bool,
) -> dict[str, Any]:
    actual_sha = tag_commit_sha(api, repository, tag)
    if actual_sha != expected_sha:
        raise ReleaseError(f"tag {tag} points to {actual_sha}, expected {expected_sha}")
    release = unique_release(api, repository, tag)
    if not release.get("draft"):
        same_state = (
            str(release.get("body") or "") == body and bool(release.get("prerelease")) == prerelease
        )
        if not same_state:
            raise ReleaseError(
                f"release {tag} is already published with different body or prerelease state"
            )
        verify_published_assets(api, repository, release, asset_dir)
        if make_latest:
            latest = api.get(f"repos/{repository}/releases/latest")
            if int(latest.get("id", -1)) != int(release["id"]):
                raise ReleaseError(f"published release {tag} is not the latest release")
        print(f"release {tag} was already published and matches the requested state")
        return dict(release)

    upload_assets(api, repository, release, asset_dir)
    if release.get("draft"):
        release = api.patch(
            f"repos/{repository}/releases/{release['id']}",
            {
                "body": body,
                "draft": False,
                "prerelease": prerelease,
                "make_latest": "true" if make_latest else "false",
            },
        )
        print(f"published {tag}: {release.get('html_url')}")
    return dict(release)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--body", type=Path, required=True)
    parser.add_argument("--asset-dir", type=Path, required=True)
    parser.add_argument("--prerelease", action="store_true")
    parser.add_argument("--make-latest", action="store_true")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        api = GitHubApi(
            os.environ.get("GH_TOKEN", ""),
            os.environ.get("GITHUB_API_URL", "https://api.github.com"),
        )
        finalize_release(
            api=api,
            repository=args.repository,
            tag=args.tag,
            expected_sha=args.sha,
            body=args.body.read_text(),
            asset_dir=args.asset_dir,
            prerelease=args.prerelease,
            make_latest=args.make_latest,
        )
    except (OSError, ReleaseError, ValueError) as error:
        print(f"release finalization error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
