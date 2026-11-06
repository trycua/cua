"""Get the machine ready for a prompt test, and put it back afterwards.

* resolve and install the build under test (the Cua Spaces app, which ships
  the `cua` CLI);
* start a Cua Driver daemon in the desktop session;
* set up each host of the spec: its own Cua home, the canary sign-in, a join
  to the relay, a terminal window, the app;
* tear all of it down, and take every computer this run added off the canary
  account.

A host is a Cua home on this machine. The app host is the computer the
person uses; a `computer` host stands in for another computer: it runs as a
plain process (no service manager), so "turning it off" is stopping that
process, which is what the relay sees when a computer loses power.
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import tempfile
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path

from . import canary
from .mcp import Mcp, window_app, window_title, windows
from .spec import Host, Spec

APP_NAMES = ("Cua Spaces", "cua-spaces")


class SetupError(RuntimeError):
    pass


@dataclass
class HostRun:
    spec: Host
    name: str
    home: str | None
    env: dict
    notes: list[str] = field(default_factory=list)
    initial_machine_id: str = ""
    procs: list = field(default_factory=list)


@dataclass
class Install:
    app: str
    cua: str
    version: str = ""
    root: str = ""
    system: bool = False


def asset_pattern(os_name: str, kind: str) -> str:
    return {
        (
            "windows",
            "exe",
        ): r"cua-spaces-nightly-windows-x64-setup\.exe$|Cua-Spaces-Setup-.*-x64\.exe$",
        ("linux", "deb"): r"cua-spaces-nightly-linux-amd64\.deb$|cua-spaces_.*_amd64\.deb$",
        (
            "linux",
            "appimage",
        ): r"cua-spaces-nightly-linux-x86_64\.AppImage$|Cua-Spaces-.*-x86_64\.AppImage$",
        (
            "macos",
            "dmg",
        ): r"cua-spaces-nightly-darwin-universal\.dmg$|cua-spaces-.*-darwin-universal\.dmg$",
    }[(os_name, kind)]


def fetch_build(build: str, os_name: str, kind: str, dest: Path) -> Path:
    """`release:<tag>` (default release:cua-spaces-nightly), `run:<id>` (a
    CD: Cua Spaces build run's artifacts) or `file:<path>`."""
    dest.mkdir(parents=True, exist_ok=True)
    pat = re.compile(asset_pattern(os_name, kind))
    if build.startswith("file:"):
        return Path(build[5:])
    if build.startswith("release:"):
        tag = build[8:]
        names = subprocess.run(
            [
                "gh",
                "release",
                "view",
                tag,
                "-R",
                "trycua/cua",
                "--json",
                "assets",
                "--jq",
                ".assets[].name",
            ],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.split()
        picks = [n for n in names if pat.search(n)]
        if not picks:
            raise SetupError(f"release {tag} has no {os_name} {kind} asset")
        name = sorted(picks, key=lambda n: ("nightly" not in n, n))[0]
        subprocess.run(
            [
                "gh",
                "release",
                "download",
                tag,
                "-R",
                "trycua/cua",
                "-p",
                name,
                "-D",
                str(dest),
                "--clobber",
            ],
            check=True,
        )
        return dest / name
    if build.startswith("run:"):
        run_id = build[4:]
        subprocess.run(
            ["gh", "run", "download", run_id, "-R", "trycua/cua", "-D", str(dest)], check=True
        )
        for p in dest.rglob("*"):
            if p.is_file() and pat.search(p.name):
                return p
        raise SetupError(f"run {run_id} has no {os_name} {kind} artifact")
    raise SetupError(f"unknown build {build!r}: release:<tag>, run:<id> or file:<path>")


def install(target, build: str, workdir: str, local_tmp: Path) -> Install:
    os_name = target.os
    if os_name == "windows":
        setup = fetch_build(build, "windows", "exe", local_tmp)
        p = subprocess.run([str(setup), "/S"], capture_output=True, timeout=600)
        if p.returncode != 0:
            raise SetupError(f"installer exited {p.returncode}")
        base = (
            Path(
                subprocess.run(
                    ["powershell", "-NoProfile", "-Command", "$env:LOCALAPPDATA"],
                    capture_output=True,
                    text=True,
                ).stdout.strip()
            )
            / "Programs"
        )
        exe = next((p for p in base.rglob("Cua Spaces.exe")), None)
        if not exe:
            raise SetupError("Cua Spaces.exe was not installed")
        cua = next((p for p in exe.parent.rglob("cua.exe")), None)
        if not cua:
            raise SetupError("the app has no bundled cua.exe")
        return Install(
            app=str(exe), cua=str(cua), root=str(exe.parent), system=True, version=setup.name
        )
    if os_name == "linux":
        if target.ephemeral:
            deb = fetch_build(build, "linux", "deb", local_tmp)
            r = target.run(["sudo", "apt-get", "install", "-y", "-qq", str(deb)], timeout=900)
            if r.rc != 0:
                raise SetupError("apt-get install failed: " + r.err[-500:])
            root = "/opt/Cua Spaces"
            app = root + "/cua-spaces"
            system = True
        else:
            image = fetch_build(build, "linux", "appimage", local_tmp)
            remote = target.path(workdir, "app.AppImage")
            target.put(image, remote)
            r = target.run(
                [
                    "sh",
                    "-c",
                    f"cd {workdir} && chmod +x app.AppImage && ./app.AppImage --appimage-extract >/dev/null",
                ],
                timeout=600,
            )
            if r.rc != 0:
                raise SetupError("AppImage extract failed: " + r.err[-500:])
            root = target.path(workdir, "squashfs-root")
            app = target.path(root, "AppRun")
            system = False
        found = target.run(
            ["sh", "-c", f"find {_q(root)} -type f -name cua -perm -u+x | head -1"]
        ).out.strip()
        if not found:
            raise SetupError("the app has no bundled cua")
        return Install(app=app, cua=found, root=root, system=system, version=Path(str(build)).name)
    if os_name == "macos":
        if not target.ephemeral:
            raise SetupError("macOS installs only on an ephemeral runner")
        dmg = fetch_build(build, "macos", "dmg", local_tmp)
        mnt = tempfile.mkdtemp(prefix="cua-dmg-")
        subprocess.run(["hdiutil", "attach", "-nobrowse", "-mountpoint", mnt, str(dmg)], check=True)
        try:
            src = next(Path(mnt).glob("*.app"))
            dst = Path("/Applications") / src.name
            shutil.rmtree(dst, ignore_errors=True)
            subprocess.run(["ditto", str(src), str(dst)], check=True)
        finally:
            subprocess.run(["hdiutil", "detach", mnt], capture_output=True)
        cua = next((p for p in dst.rglob("cua") if p.is_file() and p.stat().st_mode & 0o100), None)
        if not cua:
            raise SetupError("the app has no bundled cua")
        return Install(app=str(dst), cua=str(cua), root=str(dst), system=True, version=dmg.name)
    raise SetupError(f"unsupported os {os_name}")


def _q(s: str) -> str:
    return "'" + s.replace("'", "'\\''") + "'"


class Machine:
    """One run's provisioned machine."""

    def __init__(
        self,
        target,
        spec: Spec,
        run_id: str,
        config: dict,
        driver_bin: str,
        local_dir: Path,
        build: str,
    ):
        self.t = target
        self.spec = spec
        self.run_id = run_id
        self.config = config
        self.driver_bin = driver_bin
        self.local_dir = local_dir
        self.build = build
        base = target.home() if target.os != "windows" else tempfile.gettempdir()
        self.workdir = target.path(base, f".cua-agentic-e2e-{run_id}")
        self.events_dir = target.path(self.workdir, "events")
        self.socket = (
            (r"\\.\pipe\cua-agentic-" + run_id)
            if target.os == "windows"
            else target.path(self.workdir, "d.sock")
        )
        self.hosts: dict[int, HostRun] = {}
        self.install: Install | None = None
        self.driver_proc = None
        self.mcp: Mcp | None = None
        self.session: canary.Session | None = None
        self.log: list[str] = []

    # ---- logging -----------------------------------------------------
    def note(self, msg: str) -> None:
        line = f"[{time.strftime('%H:%M:%S')}] {msg}"
        self.log.append(line)
        print(line, flush=True)

    # ---- driver ------------------------------------------------------
    def driver_argv(self) -> list[str]:
        argv = [self.driver_bin, "mcp", "--socket", self.socket]
        if hasattr(self.t, "_cmd"):
            return self.t._cmd(argv, None)
        return argv

    def start_driver(self) -> None:
        self.t.mkdirs(self.workdir)
        self.t.mkdirs(self.events_dir)
        self.driver_proc = self.t.spawn(
            [
                self.driver_bin,
                "serve",
                "--socket",
                self.socket,
                "--no-permissions-gate",
                "--no-overlay",
            ],
            log=self.local_dir / "driver-daemon.log" if not hasattr(self.t, "_cmd") else None,
        )
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            r = self.t.run(
                [self.driver_bin, "--socket", self.socket, "call", "get_config", "{}"], timeout=30
            )
            if r.rc == 0:
                break
            time.sleep(1)
        else:
            raise SetupError("Cua Driver did not answer within 90 s")
        self.mcp = self.open_mcp("runner")
        self.note(f"Cua Driver up on {self.t.label}")

    def open_mcp(self, name: str) -> Mcp:
        proc = self.t.popen(
            [self.driver_bin, "mcp", "--socket", self.socket],
            stderr=open(self.local_dir / f"mcp-{name}.log", "ab"),
        )
        return Mcp(proc, name=f"agentic-e2e-{name}")

    # ---- hosts -------------------------------------------------------
    def host_env(self, h: HostRun | Host, home: str | None) -> dict:
        spec = h if isinstance(h, Host) else h.spec
        env: dict = {"CUA_TELEMETRY": "0", "DO_NOT_TRACK": "1", "CUA_NO_BROWSER": "1"}
        if home:
            env["CUA_HOME"] = home
        if spec.env == "staging":
            env["CUA_ENV"] = "staging"
        else:
            # Production service names, pointed at the staging relay: the
            # canary account exists on staging only.
            env["CUA_ENV"] = None
            env.update(self.config["endpoints"])
        if self.t.os in ("linux", "macos"):
            env["CUA_CREDENTIAL_STORE"] = "file"
        return env

    def cua(self, n: int, args: list[str], timeout: float = 120):
        h = self.hosts[n]
        return self.t.run([self.install.cua, *args], env=h.env, timeout=timeout)

    def setup_hosts(self) -> None:
        for spec_host in self.spec.hosts:
            n = spec_host.id
            home: str | None
            notes = []
            if spec_host.home == "default" and self.t.ephemeral:
                home = None
            else:
                if spec_host.home == "default":
                    notes.append(
                        "default home mapped to an isolated one: this machine is not ephemeral"
                    )
                home = self.t.path(self.workdir, f"h{n}")
                self.t.mkdirs(home)
            name = f"e2e-{self.run_id}-h{n}"
            hr = HostRun(spec=spec_host, name=name, home=home, env={}, notes=notes)
            hr.env = self.host_env(spec_host, home)
            self.hosts[n] = hr
            if spec_host.signed_in:
                self.sign_in(n)
            for exp in spec_host.experiments:
                r = self.cua(n, ["experiments", "enable", exp])
                if r.rc != 0:
                    raise SetupError(
                        f"host {n}: could not turn on experiment {exp}: {r.err[-300:]}"
                    )
            if spec_host.joined:
                self.join(n)
            if spec_host.terminal:
                self.open_terminal(n)
        app_host = next(h for h in self.spec.hosts if h.role == "app")
        self.launch_app(app_host.id)

    def sign_in(self, n: int) -> None:
        """`cua auth login --remote`: the device code flow, approved by the
        canary module the way a person approves it in a browser."""
        h = self.hosts[n]
        proc = self.t.popen(
            [
                self.install.cua,
                "auth",
                "login",
                "--remote",
                "--no-browser",
                "--no-onboarding",
                "--agents",
                "none",
            ],
            env=h.env,
            stdin=subprocess.DEVNULL,
            stderr=subprocess.STDOUT,
        )
        out: list[str] = []
        found = threading.Event()
        url: list[str] = []

        def read():
            for line in proc.stdout:
                out.append(line)
                m = canary.DEVICE_URL.search(line)
                if m and not url:
                    url.append(m.group(0))
                    found.set()
            found.set()

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        found.wait(120)
        if not url:
            proc.kill()
            tail = canary.USER_CODE.sub("XXXX-XXXX", "".join(out))[-300:]
            raise SetupError(
                f"host {n}: `cua auth login --remote` printed no staging sign-in link: {tail}"
            )
        link = url[0]
        if "user_code=" not in link:
            # Only the bare verification link was printed: add the code.
            time.sleep(1)
            code = canary.USER_CODE.search("".join(out))
            if not code:
                proc.kill()
                raise SetupError(f"host {n}: `cua auth login --remote` printed no user code")
            link = link.split("?")[0] + "?user_code=" + code.group(1)
        canary.approve(link)
        try:
            proc.wait(timeout=120)
        except subprocess.TimeoutExpired:
            proc.kill()
            raise SetupError(f"host {n}: sign-in did not finish after approval")
        reader.join(timeout=5)
        if proc.returncode != 0:
            tail = canary.USER_CODE.sub("XXXX-XXXX", "".join(out))[-300:]
            raise SetupError(f"host {n}: sign-in failed: {tail}")
        self.note(f"host {n} signed in as the canary account")

    def join(self, n: int) -> None:
        h = self.hosts[n]
        args = ["host", "setup", "--name", h.name, "--json"]
        if h.spec.service == "process":
            args += ["--runner", "process", "--share", "desktop"]
        r = self.cua(n, args, timeout=240)
        if r.rc != 0:
            raise SetupError(f"host {n}: `cua host setup` failed: {(r.err or r.out)[-500:]}")
        st = self.status(n)
        h.initial_machine_id = st.get("machineId", "") if st else ""
        self.note(f"host {n} joined as {h.name} ({h.initial_machine_id or 'no id'})")

    def status(self, n: int) -> dict | None:
        r = self.cua(n, ["host", "status", "--json"], timeout=60)
        try:
            return json.loads(r.out.strip().splitlines()[0]) if r.out.strip() else None
        except (ValueError, IndexError):
            return None

    def shim_dir(self, n: int) -> str:
        return self.t.path(self.workdir, f"bin-h{n}")

    def write_shims(self, n: int) -> str:
        """A `cua` that runs this host's CLI as a plain process host, and
        `cua-e2e-power off|on` that turns the stand-in computer off and on."""
        h = self.hosts[n]
        d = self.shim_dir(n)
        py = self.t.python()
        events = self.events_dir
        shim = f"""import glob, os, subprocess, sys, time
CUA = {self.install.cua!r}
HOME = {h.home!r}
NAME = {h.name!r}
EVENTS = {events!r}
N = {n}

def event(kind):
    os.makedirs(EVENTS, exist_ok=True)
    with open(os.path.join(EVENTS, "%s-%d" % (kind, N)), "w") as f:
        f.write(str(time.time()))

def power(cmd):
    if cmd == "off":
        killed = 0
        for pidfile in glob.glob(os.path.join(HOME or "", "**", "driver.pid"), recursive=True):
            try:
                pid = int(open(pidfile).read().strip())
            except (OSError, ValueError):
                continue
            if os.name == "nt":
                subprocess.call(["taskkill", "/T", "/F", "/PID", str(pid)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            else:
                for target in (-pid, pid):
                    try:
                        os.kill(target, 9)
                    except (ProcessLookupError, PermissionError):
                        pass
            killed += 1
        event("power_off")
        print("This computer is off." if killed else "This computer was already off.")
        return 0
    if cmd == "on":
        rc = subprocess.call([CUA, "host", "setup", "--name", NAME, "--runner", "process", "--share", "desktop"])
        event("power_on")
        return rc
    print("usage: cua-e2e-power off|on")
    return 2

if __name__ == "__main__":
    if sys.argv[1:2] == ["power"]:
        sys.exit(power(sys.argv[2] if len(sys.argv) > 2 else ""))
    args = sys.argv[1:]
    if args[:2] == ["host", "setup"]:
        # This terminal stands in for another computer: a plain process host,
        # named for this run so the checks and the clean-up can find it.
        if not any(a.startswith("--runner") for a in args):
            args += ["--runner", "process"]
        if not any(a.startswith("--name") for a in args):
            args += ["--name", NAME]
    sys.exit(subprocess.call([CUA] + args))
"""
        script = self.t.path(d, "e2e_shim.py")
        self.t.write(script, shim)
        if self.t.os == "windows":
            self.t.write(self.t.path(d, "cua.cmd"), f'@"{py}" "%~dp0e2e_shim.py" %*\r\n')
            self.t.write(
                self.t.path(d, "cua-e2e-power.cmd"), f'@"{py}" "%~dp0e2e_shim.py" power %*\r\n'
            )
        else:
            self.t.write(
                self.t.path(d, "cua"),
                f'#!/bin/sh\nexec {py} "$(dirname "$0")/e2e_shim.py" "$@"\n',
                executable=True,
            )
            self.t.write(
                self.t.path(d, "cua-e2e-power"),
                f'#!/bin/sh\nexec {py} "$(dirname "$0")/e2e_shim.py" power "$@"\n',
                executable=True,
            )
        return d

    def open_terminal(self, n: int) -> None:
        h = self.hosts[n]
        title = h.spec.terminal
        d = self.write_shims(n)
        env = dict(h.env)
        if self.t.os == "windows":
            path = (
                d
                + ";"
                + subprocess.run(
                    ["cmd", "/c", "echo %PATH%"], capture_output=True, text=True
                ).stdout.strip()
            )
            env["PATH"] = path
            proc = self.t.spawn(
                ["cmd.exe", "/c", "start", title, "cmd.exe", "/K", f"title {title}"], env=env
            )
        elif self.t.os == "linux":
            env["PATH"] = f"{d}:/usr/local/bin:/usr/bin:/bin"
            # A shell that never rewrites the window title, so the agent and
            # the runner find the terminal by the title the spec gave it.
            env["PS1"] = "$ "
            env["PROMPT_COMMAND"] = ""
            shell = ["bash", "--noprofile", "--norc"]
            if self.t.run(["sh", "-c", "command -v xterm"]).rc == 0:
                argv = [
                    "xterm",
                    "-T",
                    title,
                    "-geometry",
                    "110x30",
                    "-fa",
                    "Monospace",
                    "-fs",
                    "11",
                    "-e",
                    *shell,
                ]
            else:
                argv = [
                    "xfce4-terminal",
                    "--disable-server",
                    "--title",
                    title,
                    "--geometry",
                    "110x30",
                    "-x",
                    *shell,
                ]
            proc = self.t.spawn(argv, env=env)
        else:
            raise SetupError("terminal windows are not supported on macOS yet")
        h.procs.append(proc)
        self.wait_window(
            lambda w: title.lower() in window_title(w).lower(), f"terminal {title!r}", 60
        )
        self.note(f"host {n}: terminal {title!r} open")

    def launch_app(self, n: int) -> None:
        h = self.hosts[n]
        env = dict(h.env)
        env["CUA_SPACES_USER_DATA"] = self.t.path(self.workdir, f"profile-h{n}")
        argv = [self.install.app]
        if self.t.os == "linux":
            env["ACCESSIBILITY_ENABLED"] = "1"
            env["NO_AT_BRIDGE"] = None
            argv += ["--force-renderer-accessibility"]
            if not self.install.system:
                argv += ["--no-sandbox"]
        elif self.t.os == "windows":
            argv += ["--force-renderer-accessibility"]
        elif self.t.os == "macos":
            argv = [
                "open",
                "-n",
                "-W",
                self.install.app,
                "--args",
                "--force-renderer-accessibility",
            ]
        proc = self.t.spawn(
            argv, env=env, log=self.local_dir / "app.log" if not hasattr(self.t, "_cmd") else None
        )
        h.procs.append(proc)
        self.wait_window(is_app_window, "the Cua Spaces window", 120)
        self.note(f"host {n}: Cua Spaces is open")

    def wait_window(self, pred, what: str, seconds: float) -> dict:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            for w in windows(self.mcp):
                if pred(w):
                    return w
            time.sleep(2)
        raise SetupError(f"{what} did not appear within {int(seconds)} s")

    def variables(self) -> dict[str, str]:
        v = {"run": self.run_id}
        for n, h in self.hosts.items():
            v[f"host.{n}.name"] = h.name
            v[f"host.{n}.home"] = h.home or ""
            v[f"host.{n}.initial_machine_id"] = h.initial_machine_id
            st = self.status(n) if (h.spec.signed_in or h.spec.terminal) else None
            v[f"host.{n}.machine_id"] = (st or {}).get("machineId", "") or h.initial_machine_id
        return v

    # ---- teardown ----------------------------------------------------
    def teardown(self, relay_base: str) -> list[str]:
        problems = []
        for n, h in sorted(self.hosts.items(), reverse=True):
            for p in h.procs:
                try:
                    self.t.kill_tree(p.pid)
                except Exception:
                    pass
            if h.spec.signed_in:
                r = self.cua(n, ["host", "remove", "--force", "--json"], timeout=180)
                if r.rc != 0 and "not set up" not in (r.out + r.err).lower():
                    problems.append(f"host {n}: host remove: {(r.err or r.out).strip()[-200:]}")
                self.cua(n, ["auth", "logout"], timeout=60)
        # Every computer this run added leaves the canary account, whatever
        # the test did; then stale ones from runs that died (over 3 h old).
        if self.session:
            problems += sweep(self.session, relay_base, f"e2e-{self.run_id}-")
            problems += sweep(self.session, relay_base, "e2e-", older_than_s=3 * 3600, quiet=True)
        if self.mcp:
            self.mcp.close()
        if self.driver_proc is not None:
            try:
                self.t.kill_tree(self.driver_proc.pid)
            except Exception:
                pass
        if not self.t.ephemeral:
            self.t.rmtree(self.workdir)
        return problems


def is_app_window(w: dict) -> bool:
    a, t = window_app(w).lower(), window_title(w).lower()
    return any(n.lower() in a or n.lower() in t for n in APP_NAMES)


def sweep(
    session, relay_base: str, prefix: str, older_than_s: float | None = None, quiet: bool = False
) -> list[str]:
    problems = []
    status, body = canary.relay_request(session, relay_base, "GET", "/v1/machines")
    if status != 200 or not isinstance(body, dict):
        return [] if quiet else [f"relay sweep: GET /v1/machines -> {status}"]
    now = time.time()
    for m in body.get("machines") or []:
        if not str(m.get("name", "")).startswith(prefix) or m.get("role", "owner") != "owner":
            continue
        if older_than_s is not None and (
            m.get("online") or now - float(m.get("connected_at") or 0) < older_than_s
        ):
            continue
        deadline = time.monotonic() + 60
        while True:
            st, _ = canary.relay_request(session, relay_base, "DELETE", f"/v1/machines/{m['id']}")
            if st in (200, 204, 404) or time.monotonic() > deadline:
                break
            time.sleep(5)  # still connected: the host is stopping
        if st not in (200, 204, 404) and not quiet:
            problems.append(f"relay sweep: could not remove {m.get('name')} ({st})")
    return problems
