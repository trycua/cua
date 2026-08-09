import argparse
import asyncio

from cua_cli.auth.github_wif import GitHubWifError
from cua_cli.commands import wif_token


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
