"""Ready commands: shell checks that succeed once a template's start command
is serving. Same names and shell text as e2b's, so a recipe behaves the same.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import shlex


class ReadyCmd:
    """A shell command that succeeds once the sandbox is ready."""

    def __init__(self, cmd: str):
        self.__cmd = cmd

    def get_cmd(self):
        return self.__cmd


def wait_for_port(port: int):
    """Ready once something listens on `port` (matched exactly: 80 is not 8080)."""
    return ReadyCmd(f'[ -n "$(ss -Htuln sport = :{port})" ]')


def wait_for_url(url: str, status_code: int = 200):
    """Ready once `url` answers with `status_code`."""
    return ReadyCmd(
        f'curl -s -o /dev/null -w "%{{http_code}}" {shlex.quote(url)} | grep -q "{status_code}"'
    )


def wait_for_process(process_name: str):
    """Ready once a process with this name runs."""
    return ReadyCmd(f"pgrep {shlex.quote(process_name)} > /dev/null")


def wait_for_file(filename: str):
    """Ready once the file exists."""
    return ReadyCmd(f"[ -f {shlex.quote(filename)} ]")


def wait_for_timeout(timeout: int):
    """Ready after `timeout` milliseconds (at least one second)."""
    return ReadyCmd(f"sleep {max(1, timeout // 1000)}")
