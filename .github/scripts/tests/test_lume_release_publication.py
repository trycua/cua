from __future__ import annotations

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import sys
from threading import Thread
from urllib.parse import urlsplit

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[3]
REPOSITORY = "trycua/cua"
TAG = "lume-v1.2.3"
SHA = "a" * 40
API_PATH = f"/repos/{REPOSITORY}"


@pytest.fixture
def github_api():
    release = {"id": 7, "tag_name": TAG, "draft": True, "prerelease": False, "body": ""}
    publication_requests = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def respond(self, value, status=200):
            data = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            path = urlsplit(self.path).path
            if path == f"{API_PATH}/git/ref/tags/{TAG}":
                self.respond({"object": {"type": "commit", "sha": SHA}})
            elif path == f"{API_PATH}/releases":
                self.respond([release])
            elif path == f"{API_PATH}/releases/7/assets":
                self.respond([])
            else:
                self.respond({"message": f"Unexpected GET {self.path}"}, 404)

        def do_POST(self):
            url = urlsplit(self.path)
            if url.path != f"{API_PATH}/releases/7/assets":
                self.respond({"message": f"Unexpected POST {self.path}"}, 404)
                return
            self.rfile.read(int(self.headers["Content-Length"]))
            self.respond({"id": 1}, 201)

        def do_PATCH(self):
            if self.path != f"{API_PATH}/releases/7":
                self.respond({"message": f"Unexpected PATCH {self.path}"}, 404)
                return
            payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            publication_requests.append(payload)
            self.respond({**release, **payload})

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    base_url = f"http://127.0.0.1:{server.server_port}"
    release["upload_url"] = f"{base_url}{API_PATH}/releases/7/assets{{?name,label}}"
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield base_url, publication_requests
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def test_stable_lume_publication_explicitly_disables_latest_promotion(tmp_path, github_api):
    base_url, publication_requests = github_api
    workflow = yaml.safe_load((ROOT / ".github/workflows/cd-swift-lume.yml").read_text())
    commands = [
        step["run"]
        for step in workflow["jobs"]["release"]["steps"]
        if "github_release.py" in step.get("run", "")
    ]
    assert len(commands) == 1
    command = commands[0]
    for expression, value in {
        "${{ github.repository }}": REPOSITORY,
        "${{ github.ref_name }}": TAG,
        "${{ github.sha }}": SHA,
    }.items():
        command = command.replace(expression, value)
    assert "${{" not in command
    (tmp_path / ".github").symlink_to(ROOT / ".github", target_is_directory=True)
    (tmp_path / "bin").mkdir()
    (tmp_path / "bin/python3").symlink_to(sys.executable)
    (tmp_path / "release-metadata").mkdir()
    (tmp_path / "release-metadata/release-body.md").write_text("Lume release with attribution")
    (tmp_path / "release-upload").mkdir()
    (tmp_path / "release-upload/lume.tar.gz").write_bytes(b"fixture archive")

    result = subprocess.run(
        ["/bin/bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", command],
        cwd=tmp_path,
        env={
            "PATH": f"{tmp_path / 'bin'}:/usr/bin:/bin",
            "HOME": str(tmp_path),
            "GH_TOKEN": "local-fixture-token",
            "GITHUB_API_URL": base_url,
        },
        capture_output=True,
        text=True,
        timeout=30,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert publication_requests == [
        {
            "body": "Lume release with attribution",
            "draft": False,
            "prerelease": False,
            "make_latest": "false",
        }
    ]
