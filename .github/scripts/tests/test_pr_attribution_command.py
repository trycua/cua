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
UNSET = object()


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
    if isinstance(logins, dict):
        return encoded(logins)
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
        commit=None,
        message=MESSAGE,
        files=("libs/cua-driver/src/lib.rs",),
        changed_files=UNSET,
        signed=SIGNED,
        allowlist="maintainer,dependabot[bot]",
        pull_author="landing-author",
    ):
        config = tmp_path / CONFIG
        config.parent.mkdir()
        config.write_text(json.dumps({"identityOverrides": KNOWN}))
        workflow = tmp_path / ".github/workflows/cla.yml"
        workflow.parent.mkdir()
        if allowlist is not None:
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
                **(
                    {}
                    if changed_files is None
                    else {"changed_files": len(files or ()) if changed_files is UNSET else changed_files}
                ),
                "number": 50,
                "user": {"login": pull_author},
                "body": body,
                "commits": 1,
                "base": {"sha": ANCESTOR},
                "head": {"sha": HEAD, "repo": {"full_name": "contributor/cua"}},
            },
            "/repos/trycua/cua/pulls/50/commits?per_page=100&page=1": [commit or {
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
            "/repos/trycua/cua/pulls/50/files?per_page=100&page=1": (
                None if files is None else [{"filename": path} for path in files]
            ),
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

    # Informational: the finding is a warning, the check still passes.
    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning" in result.stdout
    assert "removes or changes trusted identityOverrides" in result.stdout
    assert "is merge-ready for pull request" not in result.stdout


def test_unverified_addition_on_stale_branch_is_rejected(validate_command):
    result = validate_command(ancestor={}, head={"new@institution.example": "new-author"})

    assert result.returncode == 0, result.stdout + result.stderr
    assert "new@institution.example" in result.stdout
    assert "has no explicit same-repository source PR" in result.stdout
    assert "is merge-ready for pull request" not in result.stdout


@pytest.mark.parametrize("login,warns", [("source-author", False), ("another-author", True)])
def test_addition_on_stale_branch_requires_verified_login(
    validate_command, github_api, login, warns
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

    assert result.returncode == 0, result.stdout + result.stderr
    if warns:
        assert "verified source PR author is @source-author" in result.stdout
        assert "is merge-ready for pull request" not in result.stdout
    else:
        assert "merge-ready for pull request #50" in result.stdout


@pytest.mark.parametrize("missing", ["ancestor", "head"])
def test_unreadable_configuration_is_not_approved(validate_command, missing):
    configurations = {"ancestor": {}, "head": {}}
    configurations[missing] = None
    result = validate_command(**configurations)

    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning" in result.stdout
    assert "GitHub API GET" in result.stdout
    assert "404" in result.stdout
    assert "is merge-ready for pull request" not in result.stdout


def test_missing_merge_base_is_not_approved(validate_command):
    result = validate_command(ancestor={}, head={}, merge_base_sha="")

    assert result.returncode == 0, result.stdout + result.stderr
    assert "no merge base" in result.stdout
    assert "is merge-ready for pull request" not in result.stdout


def test_unlinked_commit_email_passes_with_a_warning(validate_command):
    """A PR whose commit email GitHub cannot link never fails the check."""
    result = validate_command(
        ancestor=KNOWN,
        head=KNOWN,
        commit={
            "sha": HEAD,
            "author": None,
            "commit": {
                "author": {"name": "Some Person", "email": "private@unlinked.example"},
                "message": "docs: update guide",
            },
        },
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning" in result.stdout
    assert "private@unlinked.example" in result.stdout
    assert "informational" in result.stdout


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
            ("landing-author", "known-author"),
            MESSAGE,
            "@pull-author has not signed the CLA; sign it on #50",
        ),
        (
            ("known-author", "pull-author"),
            MESSAGE,
            "@landing-author has not signed the CLA; sign it on #50",
        ),
        (
            ("landing-author", "pull-author"),
            MESSAGE,
            "@known-author has not signed the CLA; sign it on #50",
        ),
        (
            (*SIGNED, "pull-author"),
            "fix: thing\n\nCo-authored-by: Stranger <stranger@example.com>",
            "Stranger <stranger@example.com> (no GitHub login) has not signed the CLA; "
            "sign it on #50",
        ),
    ],
    ids=["pull-author", "commit-author", "coauthor", "unmatched-coauthor"],
)
def test_unsigned_contributor_fails_on_spaces_paths(validate_command, signed, message, expected):
    result = validate_command(
        ancestor=KNOWN,
        head=KNOWN,
        files=SPACES,
        signed=signed,
        message=message,
        pull_author="pull-author",
    )

    assert result.returncode == 1
    assert "CLA is not signed" in result.stderr
    assert expected in result.stderr
    assert "merge-ready" not in result.stdout


def source_commit(index, login):
    return {
        "sha": f"{index:040x}",
        "author": {"login": login},
        "commit": {"author": {"name": login, "email": f"{login}@example.com"}},
    }


@pytest.mark.parametrize(
    "signed,unsigned",
    [
        ((*SIGNED, "source-committer"), "source-author"),
        ((*SIGNED, "source-author"), "source-committer"),
    ],
)
def test_unsigned_salvaged_pull_contributor_fails_naming_its_pull(
    validate_command, github_api, signed, unsigned
):
    _, responses = github_api
    responses["/repos/trycua/cua/pulls/12"] = {
        "number": 12, "user": {"login": "source-author"}, "commits": 1,
    }
    responses["/repos/trycua/cua/pulls/12/commits?per_page=100&page=1"] = [
        source_commit(1, "source-committer"),
    ]
    result = validate_command(
        ancestor=KNOWN, head=KNOWN, files=SPACES, body="Salvaged from #12", signed=signed
    )

    assert result.returncode == 1
    assert f"@{unsigned} has not signed the CLA; sign it on #12" in result.stderr


def test_unsigned_salvaged_commit_author_past_250_commits_fails(validate_command, github_api):
    _, responses = github_api
    source_head = "c" * 40
    responses["/repos/trycua/cua/pulls/12"] = {
        "number": 12,
        "user": {"login": "source-author"},
        "commits": 260,
        "base": {"ref": "main"},
        "head": {"sha": source_head},
    }
    commits = [source_commit(i, "source-committer") for i in range(259)]
    commits.append(source_commit(259, "late-committer"))
    for page in range(1, 4):
        responses[f"/repos/trycua/cua/compare/main...{source_head}?per_page=100&page={page}"] = {
            "total_commits": 260,
            "commits": commits[(page - 1) * 100 : page * 100],
        }
    result = validate_command(
        ancestor=KNOWN,
        head=KNOWN,
        files=SPACES,
        body="Salvaged from #12",
        signed=(*SIGNED, "source-author", "source-committer"),
    )

    assert result.returncode == 1
    assert "@late-committer has not signed the CLA; sign it on #12" in result.stderr
    assert "source-committer" not in result.stderr


def test_unsigned_contributor_on_other_paths_passes_with_warning(validate_command):
    result = validate_command(ancestor=KNOWN, head=KNOWN, signed=("known-author",))

    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning::@landing-author has not signed the CLA; sign it on #50" in result.stdout
    assert "merge-ready for pull request #50" in result.stdout


def stranger_message():
    return "fix: thing\n\nCo-authored-by: Stranger <stranger@example.com>"


LOOKUP_PROBLEMS = {
    "count-mismatch": ({"changed_files": 2}, "incomplete"),
    "changed-files-missing": ({"changed_files": None}, "incomplete"),
    "missing-filename": ({"extra_file": True}, "incomplete"),
    "signatures-unreadable": ({"signed": None}, "cla.json"),
    "signatures-malformed": ({"signed": {"other": []}}, "signedContributors"),
    "allowlist-unreadable": ({"allowlist": None}, "cla.yml"),
    "allowlist-missing-line": ({"no_allowlist_line": True}, "cla.yml"),
    "source-pull-unreadable": ({"body": "Salvaged from #12"}, "#12"),
    "unmatched-email": (
        {"message": stranger_message(), "signed": (*SIGNED,)},
        "(no GitHub login)",
    ),
}


def run_lookup_problem(validate_command, github_api, case, files):
    options, expected = LOOKUP_PROBLEMS[case]
    options = dict(options)
    files = tuple(files)
    if "extra_file" in options:
        options.pop("extra_file")
        files += (None,)
    if options.pop("no_allowlist_line", False):
        options["allowlist"] = None
    result = validate_command(
        ancestor=KNOWN, head=KNOWN, files=files, **options
    )
    return result, expected


@pytest.mark.parametrize("case", LOOKUP_PROBLEMS)
def test_cla_lookup_problem_warns_off_spaces_paths(validate_command, github_api, case):
    result, expected = run_lookup_problem(
        validate_command, github_api, case, ("libs/cua-driver/src/lib.rs",)
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning::" in result.stdout
    assert expected in result.stdout
    assert "CLA is not signed" not in result.stderr


@pytest.mark.parametrize("case", LOOKUP_PROBLEMS)
def test_cla_lookup_problem_fails_on_spaces_paths(validate_command, github_api, case):
    result, expected = run_lookup_problem(validate_command, github_api, case, SPACES)

    assert result.returncode == 1
    assert "CLA is not signed" in result.stderr
    assert expected in result.stderr


def test_unreadable_file_list_fails_on_any_path(validate_command):
    result = validate_command(ancestor=KNOWN, head=KNOWN, files=None, changed_files=1)

    assert result.returncode == 1
    assert "pulls/50/files" in result.stderr
    assert "treating it as touching Spaces paths" in result.stderr


def test_unreadable_later_file_page_fails(validate_command, github_api):
    _, responses = github_api
    responses["/repos/trycua/cua/pulls/50/files?per_page=100&page=2"] = None
    files = tuple(f"libs/cua-driver/f{i}.rs" for i in range(100))
    result = validate_command(ancestor=KNOWN, head=KNOWN, files=files)

    assert result.returncode == 1
    assert "treating it as touching Spaces paths" in result.stderr


def test_strict_cla_failure_survives_unreadable_attribution_config(validate_command):
    result = validate_command(
        ancestor=KNOWN, head=None, files=SPACES, signed=("known-author",)
    )

    assert result.returncode == 1
    assert "CLA is not signed" in result.stderr


def test_mixed_paths_are_strict(validate_command):
    result = validate_command(
        ancestor=KNOWN,
        head=KNOWN,
        files=("libs/cua-driver/src/lib.rs", *SPACES),
        signed=("known-author",),
    )

    assert result.returncode == 1
    assert "CLA is not signed" in result.stderr


def test_similar_prefix_is_not_strict(validate_command):
    result = validate_command(
        ancestor=KNOWN,
        head=KNOWN,
        files=("apps/cua-spaces-extra/x",),
        signed=("known-author",),
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "::warning::@landing-author has not signed the CLA" in result.stdout
