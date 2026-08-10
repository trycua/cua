from __future__ import annotations

from io import BytesIO
import json
from pathlib import Path
from urllib.error import HTTPError

import pytest

import github_release
from github_release import (
    GitHubApi,
    ReleaseError,
    finalize_release,
    tag_commit_sha,
    unique_release,
)


class FakeApi:
    def __init__(self, expected_sha: str) -> None:
        self.expected_sha = expected_sha
        self.deleted: list[str] = []
        self.uploaded: list[tuple[str, str]] = []
        self.patches: list[tuple[str, dict]] = []
        self.assets = [{"id": 21, "name": "old.zip", "state": "starter", "size": 0}]
        self.release = {
            "id": 7,
            "tag_name": "lume-v0.4.0",
            "draft": True,
            "prerelease": False,
            "body": "generated placeholder",
            "upload_url": "https://uploads.example/releases/7/assets{?name,label}",
            "html_url": "https://github.com/trycua/cua/releases/tag/lume-v0.4.0",
        }

    def get(self, path: str):
        if path.endswith("/git/ref/tags/lume-v0.4.0"):
            return {"object": {"type": "tag", "sha": "annotated"}}
        if path.endswith("/git/tags/annotated"):
            return {"object": {"type": "commit", "sha": self.expected_sha}}
        if "/releases?" in path:
            return [self.release] if self.release else []
        if path.endswith("/releases/7/assets?per_page=100&page=1"):
            return list(self.assets)
        if path.endswith("/releases/latest"):
            return self.release
        raise AssertionError(path)

    def delete(self, path: str):
        self.deleted.append(path)

    def upload(self, url: str, path: Path):
        self.uploaded.append((url, path.name))

    def patch(self, path: str, body: dict):
        self.patches.append((path, body))
        self.release.update(body)
        return self.release


def test_finalize_verifies_tag_replaces_incomplete_assets_then_publishes(tmp_path: Path):
    (tmp_path / "old.zip").write_bytes(b"new archive")
    (tmp_path / "new.sha256").write_text("digest")
    api = FakeApi("abc123")

    release = finalize_release(
        api=api,
        repository="trycua/cua",
        tag="lume-v0.4.0",
        expected_sha="abc123",
        body="final body",
        asset_dir=tmp_path,
        prerelease=False,
        make_latest=True,
    )

    assert tag_commit_sha(api, "trycua/cua", "lume-v0.4.0") == "abc123"
    assert api.deleted == ["repos/trycua/cua/releases/assets/21"]
    assert [name for _, name in api.uploaded] == ["new.sha256", "old.zip"]
    assert api.patches[0][1] == {
        "body": "final body",
        "draft": False,
        "prerelease": False,
        "make_latest": "true",
    }
    assert release["draft"] is False


def test_release_and_tag_invariants_fail_closed():
    api = FakeApi("actual")
    with pytest.raises(ReleaseError, match="expected expected"):
        finalize_release(
            api=api,
            repository="trycua/cua",
            tag="lume-v0.4.0",
            expected_sha="expected",
            body="body",
            asset_dir=Path("unused"),
            prerelease=False,
            make_latest=True,
        )
    api.release = None
    with pytest.raises(ReleaseError, match="does not exist"):
        unique_release(api, "trycua/cua", "lume-v0.4.0")


def test_published_release_is_verified_without_mutation(tmp_path: Path):
    artifact = tmp_path / "lume.tar.gz"
    artifact.write_bytes(b"archive")
    api = FakeApi("abc123")
    api.release.update({"draft": False, "body": "final body"})
    api.assets = [
        {
            "id": 22,
            "name": "lume.tar.gz",
            "state": "uploaded",
            "size": artifact.stat().st_size,
        }
    ]

    finalize_release(
        api=api,
        repository="trycua/cua",
        tag="lume-v0.4.0",
        expected_sha="abc123",
        body="final body",
        asset_dir=tmp_path,
        prerelease=False,
        make_latest=True,
    )

    assert api.uploaded == []
    assert api.deleted == []
    assert api.patches == []


def test_github_api_retries_transient_server_error(monkeypatch: pytest.MonkeyPatch):
    class Response(BytesIO):
        status = 200

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            self.close()

    responses = iter(
        [
            HTTPError(
                "https://api.github.com/repos/trycua/cua/releases",
                503,
                "Service Unavailable",
                {"Retry-After": "0.25"},
                BytesIO(b"temporary GitHub error"),
            ),
            Response(json.dumps({"ok": True}).encode()),
        ]
    )
    sleeps: list[float] = []

    def fake_urlopen(*_args, **_kwargs):
        response = next(responses)
        if isinstance(response, Exception):
            raise response
        return response

    monkeypatch.setattr(github_release, "urlopen", fake_urlopen)
    monkeypatch.setattr(github_release.time, "sleep", sleeps.append)

    api = GitHubApi("token", max_attempts=3, retry_base_seconds=0.1)
    assert api.get("repos/trycua/cua/releases") == {"ok": True}
    assert sleeps == [0.25]
