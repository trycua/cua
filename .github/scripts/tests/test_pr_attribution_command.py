from __future__ import annotations

import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
from threading import Thread

import pytest


ROOT = Path(__file__).resolve().parents[3]
CONFIG = ".github/release-attribution-config.json"
ANCESTOR = "a" * 40
HEAD = "b" * 40
KNOWN = {"known@institution.example": "known-author"}
SIGNED = ("landing-author", "known-author")
MESSAGE = "docs: update guide\n\nCo-authored-by: Known <known@institution.example>"
SPACES = ("apps/cua-spaces/src/main.ts",)


def encoded(value):
    data = json.dumps(value).encode()
    return {"encoding": "base64", "content": base64.b64encode(data).decode()}


def contents(overrides):
    if overrides is None:
        return None
    return encoded({"identityOverrides": overrides})


def signatures(logins):
    if logins is None:
        return None
    return encoded(
        {"signedContributors": [{"name": login, "id": i} for i, login in enumerate(logins)]}
    )


@pytest.fixture
def github_api():
    responses = {}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            payload = responses.get(self.path)
            data = json.dumps(payload).encode()
            self.send_response(404 if payload is None else 200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}", responses
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


@pytest.fixture
def validate_command(tmp_path, github_api):
    api_url, responses = github_api
    env = {
        "HOME": str(tmp_path),
        "PATH": os.defpath,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GH_TOKEN": "local-fixture-token",
        "GITHUB_API_URL": api_url,
    }

    def git(*args):
        return subprocess.check_output(
            ["git", *args], cwd=tmp_path, env=env, text=True
        ).strip()

    def validate(
        *,
        ancestor,
        head,
        body="",
        merge_base_sha=ANCESTOR,
        message=MESSAGE,
        files=("libs/cua-driver/src/lib.rs",),
        changed_files=None,
        signed=SIGNED,
        allowlist="maintainer,dependabot[bot]",
    ):
        config = tmp_path / CONFIG
        config.parent.mkdir()
        config.write_text(json.dumps({"identityOverrides": KNOWN}))
        workflow = tmp_path / ".github/workflows/cla.yml"
        workflow.parent.mkdir()
        workflow.write_text(f'        with:\n          allowlist: "{allowlist}"\n')
        git("init", "-q", "--template=")
        git("add", CONFIG)
        git(
            "-c", "user.name=Fixture",
            "-c", "user.email=fixture@example.invalid",
            "commit", "-qm", "trusted policy",
        )
        trusted_sha = git("rev-parse", "HEAD")
        (tmp_path / "event.json").write_text(json.dumps({
            "repository": {"full_name": "trycua/cua"},
            "pull_request": {"number": 50},
        }))
        responses.update({
            "/repos/trycua/cua/pulls/50": {
                "number": 50,
                "user": {"login": "landing-author"},
                "body": body,
                "commits": 1,
                "changed_files": len(files) if changed_files is None else changed_files,
                "base": {"sha": ANCESTOR},
                "head": {"sha": HEAD, "repo": {"full_name": "contributor/cua"}},
            },
            "/repos/trycua/cua/pulls/50/commits?per_page=100&page=1": [{
                "sha": HEAD,
                "author": {"login": "landing-author"},
                "commit": {
                    "author": {
                        "name": "Landing Author",
                        "email": "123+landing-author@users.noreply.github.com",
                    },
                    "message": message,
                },
            }],
            "/repos/trycua/cua/pulls/50/files?per_page=100&page=1": [
                {"filename": path} for path in files
            ],
            "/repos/trycua/cua/contents/signatures/version1/cla.json?ref=cla-signatures": (
                signatures(signed)
            ),
            f"/repos/contributor/cua/contents/{CONFIG}?ref={HEAD}": contents(head),
            f"/repos/trycua/cua/compare/{trusted_sha}...{HEAD}?per_page=1": {
                "merge_base_commit": {"sha": merge_base_sha},
            },
            f"/repos/trycua/cua/contents/{CONFIG}?ref={ANCESTOR}": contents(ancestor),
        })
        return subprocess.run(
            [
                sys.executable, str(ROOT / ".github/scripts/release_attribution.py"),
                "validate-pr", "--event", "event.json",
            ],
            cwd=tmp_path,
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )

    return validate


@pytest.mark.parametrize(
    "ancestor",
    [
        {},
        {"known@institution.example": "previous-author"},
        {"retired@institution.example": "retired-author"},
        KNOWN,
    ],
    ids=["before-addition", "before-update", "before-removal", "current"],
)
def test_unchanged_configuration_uses_current_trusted_policy(validate_command, ancestor):
    result = validate_command(ancestor=ancestor, head=ancestor)

    assert result.returncode == 0, result.stdout + result.stderr
    assert "merge-ready for pull request #50" in result.stdout


@pytest.mark.parametrize(
    "head", [{}, {"known@institution.example": "other-author"}], ids=["removed", "altered"]
)
def test_actual_changes_to_trusted_mappings_are_rejected(validate_command, head):
    result = validate_command(ancestor=KNOWN, head=head)

    assert result.returncode == 1
    assert "removes or changes trusted identityOverrides" in result.stderr
    assert "merge-ready" not in result.stdout


def test_unverified_addition_on_stale_branch_is_rejected(validate_command):
    result = validate_command(ancestor={}, head={"new@institution.example": "new-author"})

    assert result.returncode == 1
    assert "new@institution.example" in result.stderr
    assert "has no explicit same-repository source PR" in result.stderr
    assert "merge-ready" not in result.stdout


@pytest.mark.parametrize("login,exit_code", [("source-author", 0), ("another-author", 1)])
def test_addition_on_stale_branch_requires_verified_login(
    validate_command, github_api, login, exit_code
):
    _, responses = github_api
    responses["/repos/trycua/cua/pulls/12"] = {
        "number": 12, "user": {"login": "source-author"},
    }
    responses["/repos/trycua/cua/pulls/12/commits?per_page=100&page=1"] = [
        {"commit": {"author": {"email": "source@institution.example"}}},
    ]
    result = validate_command(
        ancestor={},
        head={"source@institution.example": login},
        body="The identity is verified by https://github.com/trycua/cua/pull/12",
    )

    assert result.returncode == exit_code, result.stdout + result.stderr
    if exit_code:
        assert "verified source PR author is @source-author" in result.stderr
        assert "merge-ready" not in result.stdout
    else:
        assert "merge-ready for pull request #50" in result.stdout


@pytest.mark.parametrize("missing", ["ancestor", "head"])
def test_unreadable_configuration_is_not_approved(validate_command, missing):
    configurations = {"ancestor": {}, "head": {}}
    configurations[missing] = None
    result = validate_command(**configurations)

    assert result.returncode == 1
    assert "GitHub API GET" in result.stderr
    assert "404" in result.stderr
    assert "merge-ready" not in result.stdout


def test_missing_merge_base_is_not_approved(validate_command):
    result = validate_command(ancestor={}, head={}, merge_base_sha="")

    assert result.returncode == 1
    assert "no merge base" in result.stderr
    assert "merge-ready" not in result.stdout


def test_signed_contributors_pass_on_spaces_paths(validate_command):
    result = validate_command(ancestor=KNOWN, head=KNOWN, files=SPACES)

    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning::" not in result.stdout
    assert "merge-ready for pull request #50" in result.stdout


def test_allowlisted_contributor_passes_without_signature(validate_command):
    result = validate_command(
        ancestor=KNOWN,
        head=KNOWN,
        files=SPACES,
        signed=("known-author",),
        allowlist="maintainer,Landing-Author",
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning::" not in result.stdout


@pytest.mark.parametrize(
    "signed,message,expected",
    [
        (
            ("known-author",),
            MESSAGE,
            "@landing-author has not signed the CLA; sign it on #50",
        ),
        (
            ("landing-author",),
            MESSAGE,
            "@known-author has not signed the CLA; sign it on #50",
        ),
        (
            SIGNED,
            "fix: thing\n\nCo-authored-by: Stranger <stranger@example.com>",
            "Stranger <stranger@example.com> (no GitHub login) has not signed the CLA; "
            "sign it on #50",
        ),
    ],
    ids=["commit-author", "coauthor", "unmatched-coauthor"],
)
def test_unsigned_contributor_fails_on_spaces_paths(validate_command, signed, message, expected):
    result = validate_command(
        ancestor=KNOWN, head=KNOWN, files=SPACES, signed=signed, message=message
    )

    assert result.returncode == 1
    assert "CLA is not signed" in result.stderr
    assert expected in result.stderr
    assert "merge-ready" not in result.stdout


def test_unsigned_salvaged_pull_author_fails_naming_its_pull(validate_command, github_api):
    _, responses = github_api
    responses["/repos/trycua/cua/pulls/12"] = {
        "number": 12, "user": {"login": "source-author"},
    }
    responses["/repos/trycua/cua/pulls/12/commits?per_page=100&page=1"] = [
        {
            "author": {"login": "source-author"},
            "commit": {"author": {"name": "Source", "email": "source@example.com"}},
        },
    ]
    result = validate_command(ancestor=KNOWN, head=KNOWN, files=SPACES, body="Salvaged from #12")

    assert result.returncode == 1
    assert "@source-author has not signed the CLA; sign it on #12" in result.stderr


def test_unsigned_contributor_on_other_paths_passes_with_warning(validate_command):
    result = validate_command(ancestor=KNOWN, head=KNOWN, signed=("known-author",))

    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning::@landing-author has not signed the CLA; sign it on #50" in result.stdout
    assert "merge-ready for pull request #50" in result.stdout


def test_incomplete_file_list_is_treated_as_strict(validate_command):
    result = validate_command(ancestor=KNOWN, head=KNOWN, signed=("known-author",), changed_files=2)

    assert result.returncode == 1
    assert "@landing-author has not signed the CLA" in result.stderr


def test_unreadable_cla_signatures_fail(validate_command):
    result = validate_command(ancestor=KNOWN, head=KNOWN, signed=None)

    assert result.returncode == 1
    assert "cla.json" in result.stderr
    assert "404" in result.stderr
    assert "merge-ready" not in result.stdout
