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


def configuration(overrides):
    return {"identityOverrides": overrides}


def contents(overrides):
    return {
        "encoding": "base64",
        "content": base64.b64encode(json.dumps(configuration(overrides)).encode()).decode(),
    }


@pytest.fixture
def github_api():
    responses = {}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            payload = responses.get(self.path, {"message": f"Unexpected GET {self.path}"})
            data = json.dumps(payload).encode()
            self.send_response(200 if self.path in responses else 404)
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

    def validate(*, ancestor, head, trusted=KNOWN, body=""):
        config = tmp_path / CONFIG
        config.parent.mkdir()
        config.write_text(json.dumps(configuration(trusted)))
        git("init", "-q", "--template=")
        git("add", CONFIG)
        git(
            "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
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
                    "message": "docs: update guide\n\nCo-authored-by: Known <known@institution.example>",
                },
            }],
            f"/repos/contributor/cua/contents/{CONFIG}?ref={HEAD}": contents(head),
            f"/repos/trycua/cua/compare/{trusted_sha}...{HEAD}?per_page=1": {
                "merge_base_commit": {"sha": ANCESTOR},
            },
            f"/repos/trycua/cua/contents/{CONFIG}?ref={ANCESTOR}": contents(ancestor),
        })
        return subprocess.run(
            [sys.executable, str(ROOT / ".github/scripts/release_attribution.py"),
             "validate-pr", "--event", "event.json"],
            cwd=tmp_path, env=env, capture_output=True, text=True, timeout=30,
        )

    return validate


def test_unchanged_stale_configuration_uses_current_trusted_policy(validate_command):
    result = validate_command(ancestor={}, head={})

    assert result.returncode == 0, result.stdout + result.stderr
    assert "merge-ready for pull request #50" in result.stdout
