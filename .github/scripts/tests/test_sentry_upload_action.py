"""The Sentry upload action never fails a release build.

Runs the composite action's script against a stub `npx` (no network), for
each kind and each delete-maps setting, and checks it exits 0 even when the
Sentry CLI fails. v0.17.0-beta.2's Electron Linux job failed on its last
line (`[ "$DELETE_MAPS" = true ] && find ...` with delete-maps off).
"""

from __future__ import annotations

import os
import pathlib
import subprocess

import pytest
import yaml

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
ACTION = REPO_ROOT / ".github/actions/sentry-upload/action.yml"


def script() -> str:
    action = yaml.safe_load(ACTION.read_text())
    return action["runs"]["steps"][0]["run"]


def run(tmp_path: pathlib.Path, *, kind: str, delete_maps: str, inject: str, cli_ok: bool) -> subprocess.CompletedProcess:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    npx = bin_dir / "npx"
    npx.write_text(f"#!/bin/sh\nexit {0 if cli_ok else 1}\n")
    npx.chmod(0o755)
    upload = tmp_path / "dist"
    upload.mkdir(exist_ok=True)
    (upload / "main.js").write_text("//")
    (upload / "main.js.map").write_text("{}")
    env = {
        **os.environ,
        "PATH": f"{bin_dir}:{os.environ['PATH']}",
        "GITHUB_WORKSPACE": str(REPO_ROOT),
        "SENTRY_AUTH_TOKEN": "test-token",
        "VERSION": "0.17.0-beta.3",
        "KIND": kind,
        "UPLOAD_PATH": str(upload),
        "URL_PREFIX": "app:///dist-electron",
        "DELETE_MAPS": delete_maps,
        "INJECT": inject,
        "SENTRY_CLI": "@sentry/cli@3.8.2",
    }
    return subprocess.run(["bash", "-c", script()], env=env, capture_output=True, text=True)


@pytest.mark.parametrize("delete_maps", ["true", "false"])
@pytest.mark.parametrize("inject", ["true", "false"])
@pytest.mark.parametrize("cli_ok", [True, False])
def test_sourcemaps_upload_never_fails_the_build(tmp_path, delete_maps, inject, cli_ok):
    result = run(tmp_path, kind="sourcemaps", delete_maps=delete_maps, inject=inject, cli_ok=cli_ok)
    assert result.returncode == 0, result.stdout + result.stderr
    maps_left = (tmp_path / "dist/main.js.map").exists()
    assert maps_left == (delete_maps != "true")


@pytest.mark.parametrize("cli_ok", [True, False])
def test_debug_files_upload_never_fails_the_build(tmp_path, cli_ok):
    result = run(tmp_path, kind="debug-files", delete_maps="false", inject="false", cli_ok=cli_ok)
    assert result.returncode == 0, result.stdout + result.stderr


def test_an_unknown_kind_is_an_error(tmp_path):
    assert run(tmp_path, kind="nope", delete_maps="false", inject="false", cli_ok=True).returncode == 1
