"""Thin Cua Fleet controller for the existing automated evaluation CLI."""

from __future__ import annotations

import asyncio
import json
import os
import shlex
import subprocess
import tarfile
import tempfile
import time
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any

from compare_drivers import (
    ComparisonConfig,
    discover_driver_releases,
    load_launch_descriptor,
    require_release,
    task_path,
)


REMOTE_WORKSPACE = "/root/cua"
REMOTE_REPO = f"{REMOTE_WORKSPACE}/benchmarks/cua-driver-bench"
REMOTE_RUNTIME = f"{REMOTE_WORKSPACE}/libs/cua-bench-runtime"
REMOTE_TASKS_ROOT = "/root/cua-driver-bench-tasks"
REMOTE_REPO_ARCHIVE = "/tmp/cua-driver-bench-repo.tar.gz"
REMOTE_DRIVER_ARCHIVE = "/tmp/cua-driver-bench-driver.tar.gz"
REMOTE_TASKS_ARCHIVE = "/tmp/cua-driver-bench-tasks.tar.gz"
REMOTE_RESULTS_ARCHIVE = "/tmp/cua-driver-bench-results.tar.gz"
REMOTE_ENV = "/tmp/cua-driver-bench.env"
REMOTE_CODEX_HOME = "/root/.cdb-codex"
REMOTE_BENCHMARK_STDOUT = "/tmp/cua-driver-bench.stdout"
REMOTE_BENCHMARK_STDERR = "/tmp/cua-driver-bench.stderr"
REMOTE_BENCHMARK_EXIT = "/tmp/cua-driver-bench.exit"
REMOTE_PROVISION_STDOUT = "/tmp/cua-driver-bench-provision.stdout"
REMOTE_PROVISION_STDERR = "/tmp/cua-driver-bench-provision.stderr"
REMOTE_PROVISION_EXIT = "/tmp/cua-driver-bench-provision.exit"
REMOTE_ARCHIVE_STDOUT = "/tmp/cua-driver-bench-archive.stdout"
REMOTE_ARCHIVE_STDERR = "/tmp/cua-driver-bench-archive.stderr"
REMOTE_ARCHIVE_EXIT = "/tmp/cua-driver-bench-archive.exit"
FLEET_IMAGE = (
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:c1e601dbb748fdc467c663136f7592e308a91a3c19c309b75261544432826a57"
)

_EXCLUDED_PARTS = {
    ".git",
    ".pytest_cache",
    ".venv",
    "__pycache__",
    "artifacts",
    "cua-drivers",
    "fleet-results",
    "node_modules",
}

_FLEET_APPLICATION_COMMANDS = (
    "google-chrome",
    "libreoffice",
    "gnucash",
)


@dataclass(frozen=True)
class _RemoteCommandResult:
    stdout: str
    stderr: str
    returncode: int


def _selected_versions(config: ComparisonConfig) -> tuple[str, ...]:
    if config.candidate is None or config.candidate == config.baseline:
        return (config.baseline,)
    return config.baseline, config.candidate


def _archive_member_allowed(path: PurePosixPath) -> bool:
    if path.parts[:1] == ("tasks",):
        return False
    if any(part in _EXCLUDED_PARTS for part in path.parts):
        return False
    return not (path.name.startswith(".env") and path.name != ".env.example")


def _repo_archive_filter(archive_root: str):
    prefix = PurePosixPath(archive_root).parts

    def archive_filter(member: tarfile.TarInfo) -> tarfile.TarInfo | None:
        path = PurePosixPath(member.name)
        relative = (
            PurePosixPath(*path.parts[len(prefix) :])
            if path.parts[: len(prefix)] == prefix
            else path
        )
        return member if _archive_member_allowed(relative) else None

    return archive_filter


def _create_repo_archive(repo_root: Path, destination: Path) -> None:
    workspace_root = repo_root.parents[1]
    runtime_root = workspace_root / "libs" / "cua-bench-runtime"
    if not runtime_root.is_dir():
        raise ValueError(f"Cua Bench Runtime source not found: {runtime_root}")
    with tarfile.open(destination, "w:gz") as archive:
        archive.add(
            repo_root,
            arcname="cua/benchmarks/cua-driver-bench",
            filter=_repo_archive_filter("cua/benchmarks/cua-driver-bench"),
        )
        archive.add(
            runtime_root,
            arcname="cua/libs/cua-bench-runtime",
            filter=_repo_archive_filter("cua/libs/cua-bench-runtime"),
        )


def _create_driver_archive(config: ComparisonConfig, destination: Path) -> None:
    releases = discover_driver_releases(config.drivers_root, "linux")
    with tarfile.open(destination, "w:gz") as archive:
        for version in _selected_versions(config):
            release = require_release(releases, version)
            archive.add(
                release.manifest,
                arcname=f"cua-drivers/{version}/release-manifest.json",
            )
            archive.add(
                release.binary,
                arcname=f"cua-drivers/{version}/binary/cua-driver",
            )
            if release.skill_source is not None:
                archive.add(
                    release.skill_source,
                    arcname=f"cua-drivers/{version}/{release.skill_source.name}",
                )


def _create_task_archive(config: ComparisonConfig, destination: Path) -> None:
    with tarfile.open(destination, "w:gz") as archive:
        for task in config.tasks:
            task_root = task_path(config.tasks_root, task).parent
            archive.add(
                task_root,
                arcname=f"shared/{task.lower()}",
                filter=lambda member: (
                    member if _archive_member_allowed(PurePosixPath(member.name)) else None
                ),
            )


def _required_fleet_applications(config: ComparisonConfig) -> tuple[str, ...]:
    required: set[str] = set()
    for task in config.tasks:
        bundle = task_path(config.tasks_root, task).parent
        descriptor = load_launch_descriptor(bundle, "linux")
        for prerequisite in descriptor.get("prerequisites", []):
            if not isinstance(prerequisite, dict):
                continue
            check = prerequisite.get("check")
            if not isinstance(check, list) or not check:
                continue
            command = check[0]
            if command in _FLEET_APPLICATION_COMMANDS:
                required.add(command)
    return tuple(command for command in _FLEET_APPLICATION_COMMANDS if command in required)


def _application_provision_command(config: ComparisonConfig) -> str | None:
    required = _required_fleet_applications(config)
    if not required:
        return None

    lines = [
        "set -eu",
        "export DEBIAN_FRONTEND=noninteractive",
        "apt_updated=false",
        "apt_update_once() {",
        '  if [ "$apt_updated" = false ]; then',
        "    apt-get update",
        "    apt_updated=true",
        "  fi",
        "}",
    ]
    if "google-chrome" in required:
        lines.extend(
            (
                "if ! command -v google-chrome >/dev/null 2>&1; then",
                "  apt_update_once",
                "  apt-get install -y --no-install-recommends ca-certificates",
                '  python3 -c "import urllib.request; '
                "urllib.request.urlretrieve("
                "'https://dl.google.com/linux/direct/"
                "google-chrome-stable_current_amd64.deb', "
                "'/tmp/google-chrome.deb')\"",
                "  apt-get install -y /tmp/google-chrome.deb",
                "  rm -f /tmp/google-chrome.deb",
                "fi",
                "google-chrome --version",
            )
        )

    flatpak_applications = {
        "libreoffice": "org.libreoffice.LibreOffice",
        "gnucash": "org.gnucash.GnuCash",
    }
    selected_flatpaks = [
        (command, flatpak_applications[command])
        for command in ("libreoffice", "gnucash")
        if command in required
    ]
    if selected_flatpaks:
        lines.extend(
            (
                "apt_update_once",
                "apt-get install -y --no-install-recommends flatpak dbus-x11",
                "flatpak remote-add --system --if-not-exists flathub "
                "https://flathub.org/repo/flathub.flatpakrepo",
            )
        )
        for command, application in selected_flatpaks:
            lines.extend(
                (
                    f"flatpak install --system -y --noninteractive flathub {application}",
                    f"flatpak override --system --socket=x11 --nosocket=wayland "
                    f"--filesystem={REMOTE_REPO} "
                    f"--filesystem={REMOTE_TASKS_ROOT} {application}",
                    f"cat >/usr/local/bin/{command} <<'EOF'",
                    "#!/bin/sh",
                    'if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then',
                    "  exec dbus-run-session -- env GDK_BACKEND=x11 "
                    f"flatpak run --system --filesystem={REMOTE_REPO} "
                    f'--filesystem={REMOTE_TASKS_ROOT} {application} "$@"',
                    "fi",
                    "exec env GDK_BACKEND=x11 "
                    f"flatpak run --system --filesystem={REMOTE_REPO} "
                    f'--filesystem={REMOTE_TASKS_ROOT} {application} "$@"',
                    "EOF",
                    f"chmod 0755 /usr/local/bin/{command}",
                    f"{command} --version",
                )
            )
    return _bash("\n".join(lines))


def _remote_cli_arguments(config: ComparisonConfig, remote_output: str) -> list[str]:
    arguments = [
        f"{REMOTE_WORKSPACE}/.fleet-venv/bin/python",
        "automated-eval/cli.py",
        "--baseline",
        config.baseline,
        "--model",
        config.model,
        "--reasoning-effort",
        config.reasoning_effort,
        "--timeout",
        str(config.timeout_seconds),
        "--platform",
        "linux",
        "--tasks-root",
        REMOTE_TASKS_ROOT,
        "--drivers-root",
        f"{REMOTE_REPO}/cua-drivers",
        "--codex",
        "codex",
        "--codex-home",
        REMOTE_CODEX_HOME,
        "--output",
        remote_output,
    ]
    if config.candidate is not None and config.candidate != config.baseline:
        arguments.extend(("--candidate", config.candidate))
    for task in config.tasks:
        arguments.extend(("--task", task))
    return arguments


def _required_environment(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise RuntimeError(f"missing required environment variable: {name}")
    return value


def _codex_version(codex: Path) -> str:
    completed = subprocess.run(
        [str(codex), "--version"],
        check=False,
        capture_output=True,
        text=True,
        timeout=15,
    )
    if completed.returncode != 0:
        raise RuntimeError("could not read the local Codex version")
    words = completed.stdout.strip().split()
    if len(words) != 2 or words[0] != "codex-cli":
        raise RuntimeError("local Codex returned an unexpected version string")
    return words[1]


def _command_error(label: str, result: Any) -> RuntimeError:
    detail = (result.stderr or result.stdout or "no command output").strip()
    if len(detail) > 4000:
        detail = detail[-4000:]
    return RuntimeError(f"{label} failed (exit {result.returncode}): {detail}")


async def _retry_transport(operation: Any, label: str) -> Any:
    for attempt in range(3):
        try:
            return await operation()
        except Exception:
            if attempt == 2:
                raise
            print(f"[fleet] {label} transport failed; retrying...")
            await asyncio.sleep(5)
    raise AssertionError("unreachable")


async def _run_checked(worker: Any, command: str, label: str, timeout: int) -> Any:
    result = await _retry_transport(lambda: worker.shell.run(command, timeout=timeout), label)
    if result.returncode != 0:
        raise _command_error(label, result)
    return result


async def _get_or_create_pool(cua_sandbox: Any, pool_name: str) -> Any:
    try:
        pool = await cua_sandbox.Pool.get(pool_name)
        print(f"[fleet] using existing pool {pool_name}")
        return pool
    except Exception:
        print(f"[fleet] pool {pool_name} is not accessible; creating it...")
    try:
        image = cua_sandbox.Image.from_registry(FLEET_IMAGE, os_type="linux", kind="vm")
        return await cua_sandbox.Pool.apply(
            image,
            name=pool_name,
            replicas=1,
            cpu=4,
            memory_mb=8192,
            services={"server": 8000},
        )
    except Exception as error:
        raise RuntimeError(
            "could not use or create the configured Fleet pool; set "
            "CUA_POOL_NAME to a unique name owned by this account"
        ) from error


async def _claim_worker(pool: Any, claim_name: str) -> Any:
    try:
        return await pool.claim(name=claim_name, time_to_start=1800)
    except Exception:
        print("[fleet] claim transport failed; retrying once...")
        await asyncio.sleep(5)
    try:
        return await pool.claim(name=claim_name, time_to_start=1800)
    except Exception as error:
        raise RuntimeError("could not claim a ready Fleet worker") from error


async def _run_background_command(
    worker: Any,
    command: str,
    timeout: int,
    *,
    label: str,
    stdout_path: str,
    stderr_path: str,
    exit_path: str,
) -> _RemoteCommandResult:
    pid_path = f"{exit_path}.pid"
    cleanup = await worker.shell.run(
        f"rm -f {stdout_path} {stderr_path} {exit_path} {exit_path}.partial {pid_path}",
        timeout=30,
    )
    if cleanup.returncode != 0:
        raise _command_error(f"{label} state cleanup", cleanup)
    script = f"""
set +e
exit_status_partial={exit_path}.partial
printf '%s\n' "$$" >{pid_path}
finish() {{
  status=$?
  printf '%s\n' "$status" >"$exit_status_partial"
  mv "$exit_status_partial" {exit_path}
}}
trap finish EXIT
{command} >{stdout_path} 2>{stderr_path}
""".strip()
    detached = f"nohup setsid -f bash -lc {shlex.quote(script)} </dev/null >/dev/null 2>&1"
    launch = await worker.shell.run(detached, background=True)
    if launch.returncode != 0:
        raise _command_error(f"{label} launch", launch)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            complete = await worker.files.exists(exit_path)
        except Exception:
            await asyncio.sleep(5)
            continue
        if complete:
            raw_status = await worker.files.read_text(exit_path)
            if not raw_status.strip():
                await asyncio.sleep(1)
                continue
            try:
                returncode = int(raw_status.strip())
            except ValueError as error:
                raise RuntimeError(f"{label} returned an invalid exit status") from error
            stdout = ""
            stderr = ""
            if returncode != 0:
                stdout = await worker.files.read_text(stdout_path)
                stderr = await worker.files.read_text(stderr_path)
            return _RemoteCommandResult(stdout, stderr, returncode)
        await asyncio.sleep(10)
    remote_pid = ""
    if await worker.files.exists(pid_path):
        remote_pid = (await worker.files.read_text(pid_path)).strip()
    if remote_pid.isdigit():
        await worker.shell.run(
            f"kill -TERM -- -{remote_pid} 2>/dev/null || "
            f"kill -TERM {remote_pid} 2>/dev/null || true",
            timeout=30,
        )
    raise RuntimeError(f"{label} timed out after {timeout} seconds")


async def _run_background_benchmark(
    worker: Any, command: str, timeout: int
) -> _RemoteCommandResult:
    return await _run_background_command(
        worker,
        command,
        timeout,
        label="benchmark command",
        stdout_path=REMOTE_BENCHMARK_STDOUT,
        stderr_path=REMOTE_BENCHMARK_STDERR,
        exit_path=REMOTE_BENCHMARK_EXIT,
    )


def _bash(script: str) -> str:
    return "bash -lc " + shlex.quote(script)


def _bootstrap_command(config: ComparisonConfig, codex_version: str) -> str:
    task_paths = " ".join(
        shlex.quote(f"{REMOTE_TASKS_ROOT}/shared/{task.lower()}") for task in config.tasks
    )
    script = f"""
set -eu
cd {shlex.quote(REMOTE_WORKSPACE)}
python3 -m venv .fleet-venv
.fleet-venv/bin/python -m pip install --disable-pip-version-check -e {shlex.quote(f"{REMOTE_RUNTIME}[fleet]")}
if ! command -v codex >/dev/null 2>&1 || ! codex --version | grep -Fqx {shlex.quote(f"codex-cli {codex_version}")}; then
  npm install --global --no-audit --no-fund {shlex.quote(f"@openai/codex@{codex_version}")}
fi
for task_dir in {task_paths}; do
  find "$task_dir/apps" -mindepth 2 -maxdepth 2 -name package-lock.json -print0 |
    while IFS= read -r -d '' package_lock; do
      (cd "$(dirname "$package_lock")" && npm ci --no-audit --no-fund)
    done
done
""".strip()
    return _bash(script)


def _driver_check_command(config: ComparisonConfig) -> str:
    checks: list[str] = ["set -eu", "export DISPLAY=:1"]
    releases = discover_driver_releases(config.drivers_root, "linux")
    for version in _selected_versions(config):
        release = require_release(releases, version)
        binary_version = release.binary_version or version
        binary = f"{REMOTE_REPO}/cua-drivers/{version}/binary/cua-driver"
        checks.extend(
            (
                f"chmod +x {shlex.quote(binary)}",
                f"version_output=$({shlex.quote(binary)} --version)",
                f"printf '%s\\n' \"$version_output\" | "
                f"grep -F {shlex.quote(binary_version)} >/dev/null",
                f"{shlex.quote(binary)} doctor",
            )
        )
    return _bash("\n".join(checks))


def _preflight_command() -> str:
    return _bash(
        """
set -eu
uname -a
test "${DISPLAY:-}" = :1
if test -S /tmp/.X11-unix/X1; then
  echo x11_transport=filesystem-socket
elif xset q >/dev/null 2>&1; then
  echo x11_transport=abstract-socket
else
  echo "X11 display :1 is unavailable" >&2
  exit 1
fi
xset q >/dev/null
pgrep -x xfce4-session >/dev/null
""".strip()
    )


def _model_endpoint_preflight_command() -> str:
    code = (
        "import os, urllib.request; "
        "url = os.environ['OPENAI_BASE_URL'].rstrip('/') + '/models'; "
        "request = urllib.request.Request(url, headers={'Authorization': "
        "'Bearer ' + os.environ['OPENAI_API_KEY']}); "
        "urllib.request.urlopen(request, timeout=30).read(1)"
    )
    return _bash(f"set -a; . {REMOTE_ENV}; set +a; python3 -c {shlex.quote(code)}")


def _safe_extract(archive_path: Path, destination: Path) -> None:
    destination_root = destination.resolve()
    with tarfile.open(archive_path, "r:gz") as archive:
        for member in archive.getmembers():
            target = (destination / member.name).resolve()
            if target != destination_root and destination_root not in target.parents:
                raise RuntimeError("result archive contains an unsafe path")
        archive.extractall(destination)


async def _download_results(
    worker: Any, remote_output: str, local_output: Path
) -> tuple[Path, Path]:
    archive = await _run_background_command(
        worker,
        f"tar -czf {REMOTE_RESULTS_ARCHIVE} -C {shlex.quote(remote_output)} .",
        300,
        label="result archive creation",
        stdout_path=REMOTE_ARCHIVE_STDOUT,
        stderr_path=REMOTE_ARCHIVE_STDERR,
        exit_path=REMOTE_ARCHIVE_EXIT,
    )
    if archive.returncode != 0:
        raise _command_error("result archive creation", archive)
    local_output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        prefix="cdb-fleet-results-",
        suffix=".tar.gz",
        dir=local_output.parent,
        delete=False,
    ) as temporary:
        archive_path = Path(temporary.name)
    try:
        await _retry_transport(
            lambda: worker.files.download(REMOTE_RESULTS_ARCHIVE, archive_path),
            "result archive download",
        )
        local_output.mkdir(parents=True, exist_ok=False)
        _safe_extract(archive_path, local_output)
    finally:
        archive_path.unlink(missing_ok=True)
    json_path = local_output / "comparison.json"
    markdown_path = local_output / "comparison.md"
    if not json_path.is_file() or not markdown_path.is_file():
        raise RuntimeError("downloaded Fleet results are missing comparison files")
    return json_path, markdown_path


async def run_on_fleet(config: ComparisonConfig) -> tuple[Path, Path]:
    """Claim one Fleet worker, run the existing CLI, and download its output."""
    if config.platform != "linux":
        raise ValueError("Fleet evaluation currently supports only Linux/X11")
    if config.output.exists():
        raise ValueError(f"output directory already exists: {config.output}")

    client_id = _required_environment("CUA_CLIENT_ID")
    client_secret = _required_environment("CUA_CLIENT_SECRET")
    token_url = _required_environment("CUA_TOKEN_URL")
    fleet_base_url = _required_environment("CUA_FLEET_BASE_URL")
    pool_name = _required_environment("CUA_POOL_NAME")
    model_key = _required_environment("OPENAI_API_KEY")
    model_base_url = (
        os.environ.get("OPENAI_FLEET_BASE_URL") or _required_environment("OPENAI_BASE_URL")
    ).strip()
    os.environ.pop("FLEETS_TOKEN", None)

    try:
        import cua_sandbox
    except ImportError as error:
        raise RuntimeError(
            "Fleet mode requires Python 3.11-3.13 and the project fleet extra: "
            "python -m pip install -e 'libs/cua-bench-runtime[fleet]'"
        ) from error

    cua_sandbox.configure(
        client_id=client_id,
        client_secret=client_secret,
        token_url=token_url,
        fleet_base_url=fleet_base_url,
    )

    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    claim_name = f"cdb-{stamp.lower()}"
    remote_output = f"{REMOTE_REPO}/artifacts/automated-eval/{stamp}"
    codex_version = _codex_version(config.codex)
    trial_count = len(config.tasks) * len(_selected_versions(config))
    benchmark_timeout = int(config.timeout_seconds * trial_count + 600)
    keep_alive_minutes = benchmark_timeout / 60 + 20

    worker = None
    active_error: BaseException | None = None
    secret_written = False
    with tempfile.TemporaryDirectory(prefix="cdb-fleet-") as temporary:
        temporary_root = Path(temporary)
        repo_archive = temporary_root / "repo.tar.gz"
        driver_archive = temporary_root / "drivers.tar.gz"
        task_archive = temporary_root / "tasks.tar.gz"
        _create_repo_archive(config.repo_root, repo_archive)
        _create_driver_archive(config, driver_archive)
        _create_task_archive(config, task_archive)

        print(f"[fleet] claiming one worker from {pool_name}...")
        try:
            pool = await _get_or_create_pool(cua_sandbox, pool_name)
            worker = await _claim_worker(pool, claim_name)
            await worker.keep_alive(minutes=keep_alive_minutes)
            worker_id = worker.claim_name or worker.name or claim_name
            print(f"[fleet] claimed worker {worker_id}")

            print("[fleet] checking Linux/X11...")
            preflight = await _run_checked(
                worker,
                _preflight_command(),
                "Linux/X11 preflight",
                60,
            )
            print(preflight.stdout.strip().splitlines()[0])

            print("[fleet] uploading repository, selected tasks, and drivers...")
            await _retry_transport(
                lambda: worker.files.upload(repo_archive, REMOTE_REPO_ARCHIVE),
                "repository archive upload",
            )
            await _retry_transport(
                lambda: worker.files.upload(driver_archive, REMOTE_DRIVER_ARCHIVE),
                "driver archive upload",
            )
            await _retry_transport(
                lambda: worker.files.upload(task_archive, REMOTE_TASKS_ARCHIVE),
                "task archive upload",
            )
            await _run_checked(
                worker,
                f"rm -rf {REMOTE_WORKSPACE} {REMOTE_TASKS_ROOT}; mkdir -p /root; "
                f"tar -xzf {REMOTE_REPO_ARCHIVE} -C /root; "
                f"tar -xzf {REMOTE_DRIVER_ARCHIVE} -C {REMOTE_REPO}; "
                f"mkdir -p {REMOTE_TASKS_ROOT}; "
                f"tar -xzf {REMOTE_TASKS_ARCHIVE} -C {REMOTE_TASKS_ROOT}; "
                f"rm -f {REMOTE_REPO_ARCHIVE} {REMOTE_DRIVER_ARCHIVE} "
                f"{REMOTE_TASKS_ARCHIVE}",
                "repository staging",
                180,
            )

            applications = _required_fleet_applications(config)
            provision_command = _application_provision_command(config)
            if provision_command is not None:
                print("[fleet] provisioning selected task applications: " + ", ".join(applications))
                provision = await _run_background_command(
                    worker,
                    provision_command,
                    3600,
                    label="task application provisioning",
                    stdout_path=REMOTE_PROVISION_STDOUT,
                    stderr_path=REMOTE_PROVISION_STDERR,
                    exit_path=REMOTE_PROVISION_EXIT,
                )
                if provision.returncode != 0:
                    raise _command_error("task application provisioning", provision)

            print("[fleet] preparing Codex and task dependencies...")
            provider_config = "\n".join(
                (
                    'model_provider = "cdb-fleet"',
                    "",
                    '[model_providers."cdb-fleet"]',
                    'name = "CDB Fleet LiteLLM"',
                    f"base_url = {json.dumps(model_base_url)}",
                    'env_key = "OPENAI_API_KEY"',
                    'wire_api = "responses"',
                    "",
                )
            )
            await _run_checked(
                worker,
                f"mkdir -p {REMOTE_CODEX_HOME}",
                "Codex configuration staging",
                30,
            )
            await _retry_transport(
                lambda: worker.files.write_text(
                    f"{REMOTE_CODEX_HOME}/config.toml", provider_config
                ),
                "agent configuration upload",
            )
            await _run_checked(
                worker,
                _bootstrap_command(config, codex_version),
                "worker bootstrap",
                1200,
            )

            print("[fleet] verifying selected Cua Driver releases...")
            await _run_checked(
                worker,
                _driver_check_command(config),
                "Cua Driver verification",
                180,
            )

            await _retry_transport(
                lambda: worker.files.write_text(
                    REMOTE_ENV,
                    f"OPENAI_API_KEY={shlex.quote(model_key)}\n"
                    f"OPENAI_BASE_URL={shlex.quote(model_base_url)}\n",
                ),
                "model environment upload",
            )
            secret_written = True
            await _run_checked(worker, f"chmod 600 {REMOTE_ENV}", "secret staging", 30)
            await _run_checked(
                worker,
                _model_endpoint_preflight_command(),
                "model endpoint preflight",
                60,
            )

            remote_arguments = _remote_cli_arguments(config, remote_output)
            benchmark_command = (
                f"set -a; . {REMOTE_ENV}; set +a; export DISPLAY=:1; "
                f"cd {REMOTE_REPO}; {shlex.join(remote_arguments)}"
            )
            print("[fleet] running: " + shlex.join(remote_arguments))
            benchmark = await _run_background_benchmark(
                worker, benchmark_command, benchmark_timeout
            )

            await _run_checked(worker, f"rm -f {REMOTE_ENV}", "secret cleanup", 30)
            secret_written = False

            output_exists = await worker.files.is_dir(remote_output)
            if not output_exists:
                raise _command_error("benchmark command", benchmark)
            await _run_checked(
                worker,
                f"cp {REMOTE_BENCHMARK_STDOUT} {shlex.quote(remote_output)}/fleet.stdout; "
                f"cp {REMOTE_BENCHMARK_STDERR} {shlex.quote(remote_output)}/fleet.stderr",
                "benchmark log staging",
                30,
            )
            print("[fleet] downloading reports and raw artifacts...")
            json_path, markdown_path = await _download_results(worker, remote_output, config.output)
            if benchmark.returncode != 0:
                raise _command_error("benchmark command", benchmark)
            print("[fleet] benchmark completed")
            return json_path, markdown_path
        except BaseException as error:
            active_error = error
            raise
        finally:
            if worker is not None and secret_written:
                try:
                    await worker.shell.run(f"rm -f {REMOTE_ENV}", timeout=30)
                except Exception:
                    pass
            if worker is not None:
                print("[fleet] releasing worker...")
                try:
                    await worker.close()
                except Exception as release_error:
                    if active_error is None:
                        raise RuntimeError("could not release Fleet worker") from release_error
                    print(f"[fleet] warning: worker release failed: {release_error}")
