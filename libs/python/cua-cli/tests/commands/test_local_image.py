from argparse import Namespace
from types import SimpleNamespace

from cua_cli.commands import local_image


def test_linux_docker_shell_uses_platform_internal_ports(monkeypatch) -> None:
    calls = []

    def fake_run(command, **kwargs):
        calls.append(command)
        if command[:2] == ["docker", "ps"]:
            return SimpleNamespace(stdout="", stderr="", returncode=0)
        return SimpleNamespace(stdout="container-id\n", stderr="", returncode=0)

    monkeypatch.setattr(local_image.subprocess, "run", fake_run)

    result = local_image._shell_linux_docker(
        Namespace(vnc_port=16901, api_port=18000, detach=True),
        {"docker_image": "example.invalid/cua-linux:test"},
        "linux-docker",
    )

    assert result == 0
    assert calls[-1] == [
        "docker",
        "run",
        "-d",
        "-t",
        "--rm",
        "-p",
        "16901:6080",
        "-p",
        "18000:8000",
        "--name",
        "cua-shell-linux-docker",
        "example.invalid/cua-linux:test",
    ]
