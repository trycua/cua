"""The machine a prompt test runs on: this machine (CI runners) or one reached
over SSH (our own test VMs; POSIX only). Every command, file read and the
Cua Driver MCP connection go through a Target, so checks never care where
the machine is."""

from __future__ import annotations

import os
import platform
import shlex
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


@dataclass
class Result:
    rc: int
    out: str
    err: str


def _os_name() -> str:
    s = platform.system().lower()
    return {"darwin": "macos", "windows": "windows"}.get(s, "linux")


class LocalTarget:
    """This machine. On CI the runner is ephemeral, so default homes and
    system installs are allowed."""

    def __init__(self, ephemeral: bool, extra_env: dict[str, str] | None = None):
        self.os = _os_name()
        self.ephemeral = ephemeral
        self.extra_env = dict(extra_env or {})
        self.label = f"local {self.os}"

    def env(self, env: dict[str, str] | None) -> dict[str, str]:
        e = dict(os.environ)
        e.update(self.extra_env)
        for k, v in (env or {}).items():
            if v is None:
                e.pop(k, None)
            else:
                e[k] = v
        return e

    def run(
        self, argv: list[str], env=None, timeout: float = 120, input: str | None = None
    ) -> Result:
        try:
            p = subprocess.run(
                argv,
                env=self.env(env),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout,
                input=input,
                stdin=None if input is not None else subprocess.DEVNULL,
            )
            return Result(p.returncode, p.stdout, p.stderr)
        except subprocess.TimeoutExpired as e:
            return Result(124, _s(e.stdout), _s(e.stderr) + f"\n[timed out after {timeout}s]")
        except FileNotFoundError as e:
            return Result(127, "", str(e))

    def popen(
        self,
        argv: list[str],
        env=None,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=None,
        text=True,
    ) -> subprocess.Popen:
        kw = {}
        if self.os == "windows":
            kw["creationflags"] = 0x00000200  # CREATE_NEW_PROCESS_GROUP
        else:
            kw["start_new_session"] = True
        return subprocess.Popen(
            argv,
            env=self.env(env),
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            encoding="utf-8" if text else None,
            **kw,
        )

    def spawn(self, argv: list[str], env=None, log: Path | None = None) -> subprocess.Popen:
        """A background process (the app, a terminal) that outlives no run."""
        out = open(log, "ab") if log else subprocess.DEVNULL
        return self.popen(
            argv,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=subprocess.STDOUT,
            text=False,
        )

    def kill_tree(self, pid: int) -> None:
        if self.os == "windows":
            subprocess.run(["taskkill", "/T", "/F", "/PID", str(pid)], capture_output=True)
        else:
            try:
                os.killpg(pid, 15)
            except (ProcessLookupError, PermissionError):
                try:
                    os.kill(pid, 15)
                except ProcessLookupError:
                    pass

    def path(self, *parts: str) -> str:
        return str(Path(*parts))

    def home(self) -> str:
        return str(Path.home())

    def mkdirs(self, path: str) -> None:
        Path(path).mkdir(parents=True, exist_ok=True)

    def read(self, path: str) -> str | None:
        try:
            return Path(os.path.expanduser(path)).read_text(encoding="utf-8", errors="replace")
        except OSError:
            return None

    def write(self, path: str, text: str, executable: bool = False) -> None:
        p = Path(path)
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")
        if executable and self.os != "windows":
            p.chmod(0o755)

    def exists(self, path: str) -> bool:
        return Path(os.path.expanduser(path)).exists()

    def rmtree(self, path: str) -> None:
        shutil.rmtree(path, ignore_errors=True)

    def fetch(self, path: str, local: Path) -> bool:
        src = Path(path)
        if not src.exists():
            return False
        if src.resolve() == local.resolve():
            return True
        local.parent.mkdir(parents=True, exist_ok=True)
        if src.is_dir():
            shutil.copytree(src, local, dirs_exist_ok=True)
        else:
            shutil.copy2(src, local)
        return True

    def put(self, local: Path, path: str) -> None:
        self.fetch(str(local), Path(path))

    def python(self) -> str:
        return sys.executable


class SshTarget:
    """A POSIX machine over SSH (our Linux and macOS test VMs). Never
    ephemeral: default homes are mapped to isolated ones and nothing is
    installed system-wide."""

    def __init__(self, dest: str, os_name: str, extra_env: dict[str, str] | None = None):
        self.dest = dest
        self.os = os_name
        self.ephemeral = False
        self.extra_env = dict(extra_env or {})
        self.label = f"ssh {os_name}"
        # CUA_E2E_SSH_CONFIG: an ssh_config file for the destination (a
        # temporary key, a host alias).
        cfg = os.environ.get("CUA_E2E_SSH_CONFIG")
        self._opts = (["-F", cfg] if cfg else []) + ["-o", "BatchMode=yes"]
        self._ssh = [
            "ssh",
            *self._opts,
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ConnectTimeout=20",
            dest,
        ]
        self._home: str | None = None

    def _cmd(self, argv: list[str], env) -> list[str]:
        e = dict(self.extra_env)
        e.update(env or {})
        assigns = [f"{k}={shlex.quote(v)}" for k, v in e.items() if v is not None]
        unsets = [f"-u {k}" for k, v in e.items() if v is None]
        line = " ".join(["env", *unsets, *assigns, *map(_rq, argv)])
        return [*self._ssh, line]

    def run(self, argv, env=None, timeout: float = 120, input: str | None = None) -> Result:
        try:
            p = subprocess.run(
                self._cmd(argv, env),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout,
                input=input,
                stdin=None if input is not None else subprocess.DEVNULL,
            )
            return Result(p.returncode, p.stdout, p.stderr)
        except subprocess.TimeoutExpired as e:
            return Result(124, _s(e.stdout), _s(e.stderr) + f"\n[timed out after {timeout}s]")

    def popen(
        self, argv, env=None, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=None, text=True
    ):
        return subprocess.Popen(
            self._cmd(argv, env),
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            encoding="utf-8" if text else None,
            start_new_session=True,
        )

    def spawn(self, argv, env=None, log: Path | None = None):
        # Detached on the far side, so it survives the SSH session; the pid
        # is printed for kill_tree.
        e = dict(self.extra_env)
        e.update(env or {})
        assigns = " ".join(f"{k}={shlex.quote(v)}" for k, v in e.items() if v is not None)
        inner = f"env {assigns} {' '.join(map(_rq, argv))}"
        remote_log = "/dev/null"
        line = f"setsid nohup sh -c {shlex.quote(inner)} >{remote_log} 2>&1 </dev/null & echo $!"
        p = subprocess.run([*self._ssh, line], capture_output=True, text=True, timeout=60)
        pid = int((p.stdout.strip().splitlines() or ["0"])[-1] or 0)
        return _RemoteProc(self, pid)

    def kill_tree(self, pid: int) -> None:
        if pid > 0:
            self.run(
                [
                    "sh",
                    "-c",
                    f"kill -TERM -{pid} 2>/dev/null || kill -TERM {pid} 2>/dev/null; true",
                ],
                timeout=30,
            )

    def path(self, *parts: str) -> str:
        return str(PurePosixPath(*parts))

    def home(self) -> str:
        if self._home is None:
            self._home = self.run(["sh", "-c", "echo $HOME"]).out.strip()
        return self._home

    def mkdirs(self, path: str) -> None:
        self.run(["mkdir", "-p", path])

    def read(self, path: str) -> str | None:
        if path.startswith("~/"):
            path = self.path(self.home(), path[2:])
        r = self.run(["cat", path], timeout=60)
        return r.out if r.rc == 0 else None

    def write(self, path: str, text: str, executable: bool = False) -> None:
        self.run(
            [
                "sh",
                "-c",
                f'mkdir -p "$(dirname {shlex.quote(path)})" && cat > {shlex.quote(path)}'
                + (f" && chmod 755 {shlex.quote(path)}" if executable else ""),
            ],
            input=text,
        )

    def exists(self, path: str) -> bool:
        if path.startswith("~/"):
            path = self.path(self.home(), path[2:])
        return self.run(["test", "-e", path]).rc == 0

    def rmtree(self, path: str) -> None:
        if path and path not in ("/", self.home()):
            self.run(["rm", "-rf", path], timeout=300)

    def fetch(self, path: str, local: Path) -> bool:
        if not self.exists(path):
            return False
        local.parent.mkdir(parents=True, exist_ok=True)
        p = subprocess.run(
            ["scp", "-q", "-r", *self._opts, f"{self.dest}:{path}", str(local)],
            capture_output=True,
            timeout=900,
        )
        return p.returncode == 0

    def put(self, local: Path, path: str) -> None:
        self.mkdirs(str(PurePosixPath(path).parent))
        subprocess.run(
            ["scp", "-q", "-r", *self._opts, str(local), f"{self.dest}:{path}"],
            check=True,
            timeout=900,
        )

    def python(self) -> str:
        return "python3"


class _RemoteProc:
    def __init__(self, target: SshTarget, pid: int):
        self.target, self.pid = target, pid

    def poll(self):
        return None if self.target.run(["kill", "-0", str(self.pid)]).rc == 0 else 0


def _rq(a: str) -> str:
    # Keep ~/ expandable on the far side.
    if a.startswith("~/"):
        return "~/" + shlex.quote(a[2:])
    return shlex.quote(a)


def _s(b) -> str:
    if b is None:
        return ""
    return b if isinstance(b, str) else b.decode("utf-8", "replace")


def make(spec: str | None, ephemeral: bool, extra_env: dict[str, str]) -> LocalTarget | SshTarget:
    """`local` (default) or `ssh://user@host?os=linux`."""
    if not spec or spec == "local":
        return LocalTarget(ephemeral, extra_env)
    if spec.startswith("ssh://"):
        rest = spec[len("ssh://") :]
        dest, _, query = rest.partition("?")
        os_name = "linux"
        for kv in filter(None, query.split("&")):
            k, _, v = kv.partition("=")
            if k == "os":
                os_name = v
        if os_name not in ("linux", "macos"):
            raise SystemExit("SSH targets are Linux or macOS; Windows runs on its own runner")
        return SshTarget(dest, os_name, extra_env)
    raise SystemExit(f"unknown target {spec!r}: local or ssh://user@host?os=linux")
