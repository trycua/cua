import argparse
import asyncio
from pathlib import Path

from cua_cli.auth.github_wif import GitHubWifError
from cua_cli.commands import wif_token


README = Path(__file__).parents[2] / "README.md"


def run(coroutine):
    return asyncio.run(coroutine)


def test_github_prints_only_raw_token(capsys, monkeypatch) -> None:
    async def token() -> str:
        return "header.payload.signature"

    monkeypatch.setattr(wif_token, "request_github_wif_token", token)
    monkeypatch.setattr(wif_token, "run_async", run)

    assert wif_token.cmd_github(argparse.Namespace()) == 0
    captured = capsys.readouterr()
    assert captured.out == "header.payload.signature\n"
    assert captured.err == ""


def test_github_error_has_empty_stdout(capsys, monkeypatch) -> None:
    async def token() -> str:
        raise GitHubWifError("GitHub Actions OIDC environment is unavailable.")

    monkeypatch.setattr(wif_token, "request_github_wif_token", token)
    monkeypatch.setattr(wif_token, "run_async", run)

    assert wif_token.cmd_github(argparse.Namespace()) == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert "GitHub Actions OIDC environment is unavailable" in captured.err


def test_readme_documents_github_wif_token() -> None:
    readme = README.read_text()

    assert "cua wif-token github" in readme
    assert "id-token: write" in readme
    assert "FLEETS_TOKEN" in readme
    assert "ACTIONS_ID_TOKEN_REQUEST_URL" not in readme
