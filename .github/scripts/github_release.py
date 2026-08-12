#!/usr/bin/env python3
"""Upload assets to a Release Please draft and publish it exactly once."""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
from pathlib import Path
import sys
import time
from typing import Any, Mapping, Sequence
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen

import release_channels


class ReleaseError(RuntimeError):
    """A release lifecycle invariant failed."""


class GitHubApi:
    RETRYABLE_STATUS_CODES = {429, 500, 502, 503, 504}

    def __init__(
        self,
        token: str,
        api_url: str = "https://api.github.com",
        *,
        max_attempts: int = 4,
        retry_base_seconds: float = 1.0,
    ) -> None:
        if not token:
            raise ReleaseError("GH_TOKEN is required")
        if max_attempts < 1:
            raise ValueError("max_attempts must be at least 1")
        self.token = token
        self.api_url = api_url.rstrip("/")
        self.max_attempts = max_attempts
        self.retry_base_seconds = retry_base_seconds

    def _retry_delay(self, error: HTTPError | None, attempt: int) -> float:
        if error is not None:
            retry_after = error.headers.get("Retry-After")
            if retry_after:
                try:
                    return max(0.0, float(retry_after))
                except ValueError:
                    pass
        return self.retry_base_seconds * (2 ** (attempt - 1))

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
        for attempt in range(1, self.max_attempts + 1):
            try:
                with urlopen(request, timeout=120) as response:
                    if response.status == 204:
                        return None
                    return json.load(response)
            except HTTPError as error:
                detail = error.read().decode("utf-8", errors="replace")
                retryable = error.code in self.RETRYABLE_STATUS_CODES
                if retryable and attempt < self.max_attempts:
                    delay = self._retry_delay(error, attempt)
                    print(
                        f"GitHub API {method} {path} returned {error.code}; "
                        f"retrying in {delay:g}s ({attempt}/{self.max_attempts})",
                        file=sys.stderr,
                    )
                    time.sleep(delay)
                    continue
                raise ReleaseError(
                    f"GitHub API {method} {path} failed: {error.code} {detail}"
                ) from error
            except (URLError, TimeoutError) as error:
                if attempt < self.max_attempts:
                    delay = self._retry_delay(None, attempt)
                    print(
                        f"GitHub API {method} {path} failed temporarily; "
                        f"retrying in {delay:g}s ({attempt}/{self.max_attempts}): {error}",
                        file=sys.stderr,
                    )
                    time.sleep(delay)
                    continue
                raise ReleaseError(
                    f"GitHub API {method} {path} failed after {self.max_attempts} attempts: {error}"
                ) from error
        raise AssertionError("GitHub API retry loop exited unexpectedly")

    def get(self, path: str) -> Any:
        return self.request("GET", path)

    def delete(self, path: str) -> None:
        self.request("DELETE", path)

    def post(self, path: str, body: Mapping[str, Any]) -> Any:
        return self.request("POST", path, json_body=body)

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


def validate_registered_nightly_tag(tag: str) -> None:
    # Driver publication runs after its exact nightly version has been staged
    # into the checkout. Tag validation needs registry shape and namespaces,
    # not the stable source-state invariant enforced by planning and builds.
    registry = release_channels.load_registry(require_stable_state=False)
    matches = []
    for name, component in registry["components"].items():
        try:
            release_channels.parse_tag(component, "nightly", tag)
        except release_channels.ChannelError:
            continue
        matches.append(name)
    if len(matches) != 1:
        raise ReleaseError(
            f"nightly tag {tag!r} must match exactly one registered component; matched {matches}"
        )


def ensure_release(
    api: GitHubApi,
    repository: str,
    tag: str,
    expected_sha: str,
    *,
    channel: str,
    create_if_missing: bool,
) -> dict[str, Any]:
    matches = releases_by_tag(api, repository, tag)
    if len(matches) > 1:
        ids = [release.get("id") for release in matches]
        raise ReleaseError(f"expected one GitHub release for {tag}, found {ids}")
    if matches:
        return matches[0]
    if not create_if_missing:
        raise ReleaseError(f"Release Please draft for {tag} does not exist")
    if channel != "nightly":
        raise ReleaseError("automatic draft creation is allowed only for the nightly channel")
    validate_registered_nightly_tag(tag)
    if not release_channels.SHA_RE.fullmatch(expected_sha):
        raise ReleaseError("nightly draft target must be an exact lowercase commit SHA")
    commit = api.get(f"repos/{repository}/commits/{expected_sha}")
    if str(commit.get("sha")) != expected_sha:
        raise ReleaseError(f"GitHub did not resolve expected commit {expected_sha}")
    try:
        created = api.post(
            f"repos/{repository}/releases",
            {
                "tag_name": tag,
                "target_commitish": expected_sha,
                "name": tag,
                "body": "",
                "draft": True,
                "prerelease": True,
                "make_latest": "false",
            },
        )
    except ReleaseError:
        # A retry or concurrent job may have committed the draft after the
        # initial read. Reconcile by identity; every other error remains fatal.
        matches = releases_by_tag(api, repository, tag)
        if len(matches) != 1:
            raise
        created = matches[0]
    if str(created.get("tag_name")) != tag or not created.get("draft"):
        raise ReleaseError(f"GitHub did not create the expected draft for {tag}")
    print(f"created private nightly draft for {tag}")
    return dict(created)


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
    channel: str = "stable",
    create_if_missing: bool = False,
) -> dict[str, Any]:
    if channel not in {"stable", "nightly"}:
        raise ReleaseError(f"unsupported release channel: {channel}")
    if channel == "nightly" and (not prerelease or make_latest):
        raise ReleaseError("nightly releases must be prereleases and must not become latest")
    release: dict[str, Any] | None = None
    if create_if_missing:
        release = ensure_release(
            api,
            repository,
            tag,
            expected_sha,
            channel=channel,
            create_if_missing=True,
        )
    if release is None:
        actual_sha = tag_commit_sha(api, repository, tag)
        if actual_sha != expected_sha:
            raise ReleaseError(f"tag {tag} points to {actual_sha}, expected {expected_sha}")
        release = unique_release(api, repository, tag)
    elif release.get("draft"):
        target = str(release.get("target_commitish") or "")
        if target != expected_sha:
            raise ReleaseError(
                f"nightly draft {tag} targets {target or '<missing>'}, expected {expected_sha}"
            )
    else:
        actual_sha = tag_commit_sha(api, repository, tag)
        if actual_sha != expected_sha:
            raise ReleaseError(f"tag {tag} points to {actual_sha}, expected {expected_sha}")
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
        actual_sha = tag_commit_sha(api, repository, tag)
        if actual_sha != expected_sha:
            raise ReleaseError(
                f"published tag {tag} points to {actual_sha}, expected {expected_sha}"
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
    parser.add_argument("--channel", choices=("stable", "nightly"), default="stable")
    parser.add_argument("--create-if-missing", action="store_true")
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
            channel=args.channel,
            create_if_missing=args.create_if_missing,
        )
    except (OSError, ReleaseError, ValueError) as error:
        print(f"release finalization error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
