"""Shell command execution."""

import subprocess
from dataclasses import dataclass


@dataclass
class CommandResult:
    stdout: str
    stderr: str
    returncode: int

    @property
    def success(self) -> bool:
        return self.returncode == 0


def run(command: str, timeout: int = 30) -> CommandResult:
    """Run a shell command and return stdout, stderr, and returncode.

    The command is passed to the system shell (``shell=True``) so that
    shell built-ins, pipes, and redirections work as expected.
    """

    def _decode(data: bytes) -> str:
        if not data:
            return ""
        for enc in ("utf-8", "gbk", "gb2312", "cp936", "latin1"):
            try:
                return data.decode(enc)
            except (UnicodeDecodeError, LookupError):
                continue
        return data.decode("utf-8", errors="replace")

    result = subprocess.run(
        command,
        shell=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    return CommandResult(
        stdout=_decode(result.stdout),
        stderr=_decode(result.stderr),
        returncode=result.returncode,
    )
