"""Local, non-certifying Cua Driver release comparison orchestration."""

from __future__ import annotations

import base64
import ctypes
import ctypes.util
import json
import os
import queue
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import tomllib
import urllib.error
import urllib.request
import uuid
from collections.abc import Callable, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any

from cua_bench_runtime import engine
from cua_bench_runtime.adapters.agent_harnesses.production import (
    BundledSkillFile,
    HarnessRenderContext,
    ModelRoute,
    NativeMcpDriver,
    production_harness,
)
from cua_bench_runtime.adapters.cua_recording import (
    CuaRecordingObserver,
    McpClient,
    merge_app_maps,
)
from cua_bench_runtime.adapters.local import adapters as local_adapters
from cua_bench_runtime.canon import digest_file
from cua_bench_runtime.errors import (
    DeadlineExceeded,
    HardAbort,
    HarnessFailure,
    TrialInterrupted,
    ValidationFailure,
)
from cua_bench_runtime.model import AgentOutcome, EnvironmentHandle, ObserverReport, TrialContext
from cua_bench_runtime.process import clean_environment, command_for, run_process
from cua_bench_runtime.signals import InterruptFlag


SHARED_TASKS = ("CDB-S01", "CDB-S02", "CDB-S03", "CDB-S04")
INPUT_ACTIONS = frozenset(
    {
        "browser_click",
        "browser_pointer",
        "browser_set_input_files",
        "browser_type",
        "click",
        "double_click",
        "drag",
        "hotkey",
        "press_key",
        "right_click",
        "scroll",
        "set_value",
        "type_text",
    }
)
LEGACY_PAGE_INPUT_ACTIONS = frozenset({"click_element", "insert_text", "type_keystrokes"})
GUI_ENVIRONMENT = (
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
)
SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
PLACEHOLDER = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}")
DEFAULT_BASELINE = "0.22.2"
DEFAULT_CANDIDATE = "0.23.2"
FOREGROUND_SAMPLE_HZ = 5
CURSOR_DEVIATION_THRESHOLD_PX = 10
FOREGROUND_DISTURBANCE_MEASUREMENT = "x11_controlled_focus_drag_cursor_v2"
_X11_NONE = 0
_X11_POINTER_ROOT = 1
_X11_IS_VIEWABLE = 2
_X11_BUTTON_MASK = sum(1 << bit for bit in range(8, 13))
_XRECORD_FROM_SERVER = 0
_XRECORD_FROM_CLIENT = 1
_XRECORD_ALL_CLIENTS = 3


@dataclass(frozen=True)
class DriverRelease:
    version: str
    root: Path
    binary: Path
    manifest: Path
    skill_kind: str | None = None
    skill_source: Path | None = None


@dataclass(frozen=True)
class ComparisonConfig:
    repo_root: Path
    tasks_root: Path
    drivers_root: Path
    baseline: str
    candidate: str | None
    tasks: tuple[str, ...]
    output: Path
    platform: str
    codex: Path
    codex_home: Path
    model: str
    reasoning_effort: str
    timeout_seconds: float


@dataclass(frozen=True)
class TrialMetrics:
    task: str
    version: str
    trial_id: str
    trial_dir: str | None
    passed: bool
    score: float | None
    total_ms: int | None
    cua_calls: int
    input_actions: int
    termination: str
    codex_tokens: Mapping[str, int] | None = None
    error: str | None = None
    foreground_disturbance_available: bool = False
    foreground_disturbance_measurement: str | None = None
    foreground_disturbance_error: str | None = None
    foreground_keyboard_focus_available: bool = False
    foreground_keyboard_focus_error: str | None = None
    foreground_keyboard_focus_drops: int | None = None
    foreground_drag_available: bool = False
    foreground_drag_error: str | None = None
    foreground_drag_interruptions: int | None = None
    foreground_drag_observations: int | None = None
    foreground_cursor_trajectory_available: bool = False
    foreground_cursor_trajectory_error: str | None = None
    foreground_cursor_trajectory_deviations: int | None = None
    foreground_disturbances: int | None = None
    focus_window_transitions: int | None = None
    cursor_deviation_episodes: int | None = None


@dataclass(frozen=True)
class ForegroundEvent:
    kind: str
    observed_at: str
    monotonic_ns: int
    server_time_ms: int | None = None
    client_id: int | None = None
    window: int | None = None
    button: int | None = None


@dataclass(frozen=True)
class ForegroundSample:
    focus_window: int
    cursor_x: int
    cursor_y: int
    button_mask: int = 0
    focus_window_viewable: bool = True
    pointer_window: int = 0
    pointer_window_viewable: bool = False
    observed_at: str | None = None
    monotonic_ns: int | None = None
    events: tuple[ForegroundEvent, ...] = ()


def _x11_button_bit(button: int) -> int:
    if not 1 <= button <= 5:
        return 0
    return 1 << (button + 7)


def _valid_focus(sample: ForegroundSample) -> bool:
    return (
        sample.focus_window not in {_X11_NONE, _X11_POINTER_ROOT} and sample.focus_window_viewable
    )


class _ForegroundDisturbanceCounter:
    def __init__(self, cursor_threshold_px: int) -> None:
        self.cursor_threshold_squared = cursor_threshold_px**2
        self.sample_count = 0
        self.foreground_keyboard_focus_drops = 0
        self.foreground_drag_interruptions = 0
        self.foreground_drag_observations = 0
        self.foreground_cursor_trajectory_deviations = 0
        self.focus_window_transitions = 0
        self.initial_cursor: tuple[int, int] | None = None
        self.previous_focus_window: int | None = None
        self.previous_focus_valid: bool | None = None
        self.focus_drop_active = False
        self.cursor_outside_reference = False
        self.previous_button_mask = 0
        self.drag_active = False
        self.drag_rearm_blocked = False
        self.drag_button_mask = 0
        self.drag_focus_window: int | None = None
        self.drag_window: int | None = None

    def _transition(self, metric: str, reason: str, **detail: Any) -> dict[str, Any]:
        return {"metric": metric, "reason": reason, **detail}

    def _end_drag(self) -> None:
        self.drag_active = False
        self.drag_button_mask = 0
        self.drag_focus_window = None
        self.drag_window = None

    def _interrupt_drag(self, reason: str) -> dict[str, Any]:
        transition = self._transition(
            "foreground_drag_interruptions",
            reason,
            focus_window=self.drag_focus_window,
            drag_window=self.drag_window,
            button_mask=self.drag_button_mask,
        )
        self.foreground_drag_interruptions += 1
        self.drag_rearm_blocked = True
        self._end_drag()
        return transition

    def add(self, sample: ForegroundSample) -> list[dict[str, Any]]:
        transitions: list[dict[str, Any]] = []
        focus_valid = _valid_focus(sample)
        buttons = sample.button_mask & _X11_BUTTON_MASK
        if self.sample_count == 0:
            self.initial_cursor = (sample.cursor_x, sample.cursor_y)
            self.previous_focus_window = sample.focus_window
            self.previous_focus_valid = focus_valid
            self.previous_button_mask = buttons
            self.sample_count = 1
            return transitions

        if (
            self.previous_focus_valid
            and focus_valid
            and sample.focus_window != self.previous_focus_window
        ):
            self.focus_window_transitions += 1
        if self.previous_focus_valid and not focus_valid and not self.focus_drop_active:
            self.foreground_keyboard_focus_drops += 1
            self.focus_drop_active = True
            transitions.append(
                self._transition(
                    "foreground_keyboard_focus_drops",
                    "valid_focus_became_unavailable",
                    previous_focus_window=self.previous_focus_window,
                )
            )
        elif focus_valid:
            self.focus_drop_active = False

        assert self.initial_cursor is not None
        delta_x = sample.cursor_x - self.initial_cursor[0]
        delta_y = sample.cursor_y - self.initial_cursor[1]
        outside_reference = delta_x * delta_x + delta_y * delta_y > self.cursor_threshold_squared
        if outside_reference and not self.cursor_outside_reference:
            self.foreground_cursor_trajectory_deviations += 1
            transitions.append(
                self._transition(
                    "foreground_cursor_trajectory_deviations",
                    "cursor_left_static_reference",
                    cursor_x=sample.cursor_x,
                    cursor_y=sample.cursor_y,
                )
            )
        self.cursor_outside_reference = outside_reference

        event_kinds = {event.kind for event in sample.events}
        release_mask = 0
        press_windows: list[int] = []
        unavailable_windows: set[int] = set()
        for event in sample.events:
            if event.kind == "button_release" and event.button is not None:
                release_mask |= _x11_button_bit(event.button)
            elif event.kind == "button_press" and event.window is not None:
                press_windows.append(event.window)
            elif event.kind in {"window_destroy", "window_unmap"}:
                if event.window is not None:
                    unavailable_windows.add(event.window)

        if self.drag_active:
            normal_release = (
                buttons == 0
                and self.drag_button_mask != 0
                and release_mask & self.drag_button_mask == self.drag_button_mask
            )
            if normal_release:
                transitions.append(
                    self._transition(
                        "foreground_drag_interruptions",
                        "normal_release_excluded",
                        counted=False,
                    )
                )
                self._end_drag()
            elif self.drag_window in unavailable_windows:
                transitions.append(self._interrupt_drag("drag_window_unavailable"))
            elif not focus_valid:
                transitions.append(self._interrupt_drag("foreground_focus_lost"))
            elif sample.focus_window != self.drag_focus_window:
                transitions.append(self._interrupt_drag("foreground_focus_transferred"))
            elif "pointer_ungrab" in event_kinds and buttons != 0:
                transitions.append(self._interrupt_drag("pointer_capture_lost"))
            elif buttons & self.drag_button_mask != self.drag_button_mask:
                transitions.append(self._interrupt_drag("unexpected_button_state_change"))

        if self.drag_rearm_blocked and buttons == 0:
            self.drag_rearm_blocked = False
        if (
            not self.drag_active
            and not self.drag_rearm_blocked
            and self.previous_button_mask == 0
            and buttons != 0
            and focus_valid
        ):
            press_window = next(
                (
                    window
                    for window in reversed(press_windows)
                    if window not in {_X11_NONE, _X11_POINTER_ROOT}
                ),
                None,
            )
            pointer_window = (
                sample.pointer_window
                if sample.pointer_window not in {_X11_NONE, _X11_POINTER_ROOT}
                and sample.pointer_window_viewable
                else None
            )
            self.drag_active = True
            self.drag_button_mask = buttons
            self.drag_focus_window = sample.focus_window
            self.drag_window = press_window or pointer_window or sample.focus_window
            self.foreground_drag_observations += 1
            transitions.append(
                self._transition(
                    "foreground_drag_observations",
                    "controlled_foreground_drag_observed",
                    focus_window=self.drag_focus_window,
                    drag_window=self.drag_window,
                    button_mask=self.drag_button_mask,
                )
            )

        self.previous_focus_window = sample.focus_window
        self.previous_focus_valid = focus_valid
        self.previous_button_mask = buttons
        self.sample_count += 1
        return transitions

    def finish(self) -> list[dict[str, Any]]:
        if not self.drag_active:
            return []
        self._end_drag()
        return [
            self._transition(
                "foreground_drag_interruptions",
                "observer_teardown_excluded",
                counted=False,
            )
        ]


def summarize_foreground_samples(
    samples: Sequence[ForegroundSample],
    cursor_threshold_px: int = CURSOR_DEVIATION_THRESHOLD_PX,
    *,
    drag_events_available: bool = True,
) -> dict[str, int | bool | None | str]:
    counter = _ForegroundDisturbanceCounter(cursor_threshold_px)
    for sample in samples:
        counter.add(sample)
    counter.finish()
    sample_available = counter.sample_count > 0
    focus_drops = counter.foreground_keyboard_focus_drops if sample_available else None
    drag_interruptions = (
        counter.foreground_drag_interruptions
        if sample_available and drag_events_available
        else None
    )
    cursor_deviations = (
        counter.foreground_cursor_trajectory_deviations if sample_available else None
    )
    aggregate = (
        focus_drops + drag_interruptions + cursor_deviations
        if focus_drops is not None
        and drag_interruptions is not None
        and cursor_deviations is not None
        else None
    )
    return {
        "available": aggregate is not None,
        "sample_count": counter.sample_count,
        "foreground_keyboard_focus_available": sample_available,
        "foreground_keyboard_focus_drops": focus_drops,
        "foreground_drag_available": sample_available and drag_events_available,
        "foreground_drag_interruptions": drag_interruptions,
        "foreground_drag_observations": (
            counter.foreground_drag_observations if sample_available else None
        ),
        "foreground_cursor_trajectory_available": sample_available,
        "foreground_cursor_trajectory_deviations": cursor_deviations,
        "foreground_disturbances": aggregate,
        "focus_window_transitions": (
            counter.focus_window_transitions if sample_available else None
        ),
        "cursor_deviation_episodes": cursor_deviations,
    }


def detect_platform() -> str:
    if sys.platform == "darwin":
        return "macos"
    if os.name == "nt":
        return "windows"
    return "linux"


def normalize_release_selection(
    baseline: str | None, candidate: str | None
) -> tuple[str, str | None]:
    if baseline is None and candidate is None:
        return DEFAULT_BASELINE, DEFAULT_CANDIDATE
    if baseline is None:
        assert candidate is not None
        return candidate, None
    if candidate is None or candidate == baseline:
        return baseline, None
    return baseline, candidate


def resolve_platform(value: str) -> str:
    if value == "auto":
        return detect_platform()
    if value not in {"linux", "windows", "macos"}:
        raise ValueError(f"unsupported platform: {value}")
    return value


def _semver_key(value: str) -> tuple[int, int, int]:
    match = SEMVER.fullmatch(value)
    if match is None:
        raise ValueError(f"invalid semantic version: {value}")
    return tuple(int(part) for part in match.groups())


def _driver_binary_name(platform: str) -> str:
    return "cua-driver.exe" if platform == "windows" else "cua-driver"


def _find_skill_source(root: Path) -> tuple[str | None, Path | None]:
    directory_candidates = (
        root / "skills" / "cua-driver",
        root / "skill" / "cua-driver",
        root / "cua-driver-skill",
    )
    for candidate in directory_candidates:
        if (candidate / "SKILL.md").is_file():
            return "directory", candidate
    archives = sorted(
        (
            *root.glob("*skills*.tar.gz"),
            *root.glob("*skills*.tgz"),
        ),
        key=lambda path: path.name,
    )
    if archives:
        return "archive", archives[0]
    return None, None


def discover_driver_releases(drivers_root: Path, platform: str) -> dict[str, DriverRelease]:
    root = drivers_root.resolve()
    if not root.is_dir():
        raise ValueError(f"driver root is missing: {root}")
    releases: dict[str, DriverRelease] = {}
    for directory in sorted(root.iterdir(), key=lambda path: path.name):
        if not directory.is_dir() or SEMVER.fullmatch(directory.name) is None:
            continue
        manifest_path = directory / "release-manifest.json"
        if not manifest_path.is_file():
            raise ValueError(f"release manifest is missing for {directory.name}")
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValueError(f"release manifest is invalid for {directory.name}") from error
        if manifest.get("version") != directory.name:
            raise ValueError(
                f"release directory {directory.name} does not match manifest version "
                f"{manifest.get('version')!r}"
            )
        skill_kind, skill_source = _find_skill_source(directory)
        releases[directory.name] = DriverRelease(
            version=directory.name,
            root=directory.resolve(),
            binary=(directory / "binary" / _driver_binary_name(platform)).resolve(),
            manifest=manifest_path.resolve(),
            skill_kind=skill_kind,
            skill_source=skill_source.resolve() if skill_source else None,
        )
    return dict(sorted(releases.items(), key=lambda item: _semver_key(item[0])))


def require_release(releases: Mapping[str, DriverRelease], version: str) -> DriverRelease:
    try:
        release = releases[version]
    except KeyError as error:
        available = ", ".join(releases) or "none"
        raise ValueError(f"Cua Driver {version} is unavailable; discovered: {available}") from error
    if not release.binary.is_file():
        raise ValueError(
            f"Cua Driver {version} has no binary for the selected platform: {release.binary}"
        )
    if os.name != "nt" and not os.access(release.binary, os.X_OK):
        raise ValueError(f"Cua Driver binary is not executable: {release.binary}")
    return release


def normalize_tasks(tasks: Sequence[str]) -> tuple[str, ...]:
    normalized: list[str] = []
    for value in tasks:
        task = value.upper()
        if task not in SHARED_TASKS:
            raise ValueError(
                f"unsupported shared task {value!r}; expected one of {', '.join(SHARED_TASKS)}"
            )
        if task not in normalized:
            normalized.append(task)
    return tuple(normalized)


def task_path(tasks_root: Path, task: str) -> Path:
    path = tasks_root / "shared" / task.casefold() / "task.cuabench.json"
    if not path.is_file():
        raise ValueError(f"task manifest is missing: {path}")
    return path.resolve()


def expand_placeholders(value: Any, variables: Mapping[str, str]) -> Any:
    if isinstance(value, str):

        def replace(match: re.Match[str]) -> str:
            name = match.group(1)
            if name not in variables:
                raise ValueError(f"unknown launch descriptor placeholder: {name}")
            return variables[name]

        return PLACEHOLDER.sub(replace, value)
    if isinstance(value, list):
        return [expand_placeholders(item, variables) for item in value]
    if isinstance(value, dict):
        return {key: expand_placeholders(item, variables) for key, item in value.items()}
    return value


def load_launch_descriptor(bundle: Path, platform: str) -> dict[str, Any]:
    path = bundle / "platform" / f"launch.{platform}.json"
    if not path.is_file():
        raise ValueError(f"task launch descriptor is missing: {path}")
    try:
        descriptor = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"task launch descriptor is invalid: {path}") from error
    if descriptor.get("platform") != platform:
        raise ValueError(f"task launch descriptor platform mismatch: {path}")
    if not isinstance(descriptor.get("apps"), list):
        raise ValueError(f"task launch descriptor has no app list: {path}")
    return descriptor


def _gui_environment() -> dict[str, str]:
    return {name: os.environ[name] for name in GUI_ENVIRONMENT if name in os.environ}


def _resolve_executable(command: Sequence[str]) -> list[str]:
    if not command or not all(isinstance(item, str) and item for item in command):
        raise ValueError("launch command must contain non-empty strings")
    executable = Path(command[0])
    if executable.is_absolute():
        if not executable.is_file():
            raise FileNotFoundError(command[0])
        return list(command)
    resolved = shutil.which(command[0])
    if resolved is None:
        raise FileNotFoundError(command[0])
    return [resolved, *command[1:]]


def _run_preflight_command(
    command: Sequence[str], *, cwd: Path, environment: Mapping[str, str], timeout: float
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        _resolve_executable(command),
        cwd=cwd,
        env=dict(environment),
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        check=False,
        shell=False,
    )


def _parsed_version(value: str) -> tuple[int, ...] | None:
    match = re.search(r"(?<![0-9])([0-9]+(?:\.[0-9]+)*)", value)
    if match is None:
        return None
    return tuple(int(part) for part in match.group(1).split("."))


def _version_at_least(actual: tuple[int, ...], minimum: tuple[int, ...]) -> bool:
    width = max(len(actual), len(minimum))
    return actual + (0,) * (width - len(actual)) >= minimum + (0,) * (width - len(minimum))


def _verify_driver_identity(release: DriverRelease) -> None:
    try:
        completed = _run_preflight_command(
            [str(release.binary), "--version"],
            cwd=release.root,
            environment={
                **os.environ,
                "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
            },
            timeout=10.0,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError(
            f"Cua Driver identity check failed for {release.version}: {type(error).__name__}"
        ) from error
    expected = f"cua-driver {release.version}"
    if completed.returncode != 0 or expected not in completed.stdout.strip():
        raise ValueError(
            f"Cua Driver identity check failed for {release.version}: "
            f"{completed.stdout.strip() or completed.stderr.strip() or 'no output'}"
        )


def _preflight_codex(config: ComparisonConfig) -> str:
    environment = dict(os.environ)
    environment["CODEX_HOME"] = str(config.codex_home)
    try:
        version = _run_preflight_command(
            [str(config.codex), "--version"],
            cwd=config.repo_root,
            environment=environment,
            timeout=10.0,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError(f"Codex CLI version check failed: {type(error).__name__}") from error
    if version.returncode != 0:
        raise ValueError("Codex CLI version check failed")
    return version.stdout.strip()


def _descriptor_variables(bundle: Path, workspace: Path) -> dict[str, str]:
    absolute_workspace = workspace.resolve()
    return {
        "HOME": str(Path.home()),
        "python": sys.executable,
        "bundle": str(bundle.resolve()),
        "workspace": str(absolute_workspace),
        "workspace_uri": absolute_workspace.as_uri(),
        "artifacts": str((workspace / "artifacts").resolve()),
        "result": str((workspace / "result.json").resolve()),
        "agent_exit_code": "0",
    }


def _preflight_task(bundle: Path, platform: str) -> None:
    descriptor = load_launch_descriptor(bundle, platform)
    variables = _descriptor_variables(bundle, bundle / ".preflight-workspace")
    environment = {**os.environ, **_gui_environment()}
    for prerequisite in descriptor.get("prerequisites", []):
        if not isinstance(prerequisite, dict) or not isinstance(prerequisite.get("check"), list):
            raise ValueError(f"invalid prerequisite in {bundle}")
        expanded = expand_placeholders(prerequisite, variables)
        check = list(expanded["check"])
        if expanded.get("id") == "python":
            check[0] = variables["python"]
        cwd = Path(expanded.get("cwd", str(bundle))).resolve()
        try:
            completed = _run_preflight_command(
                check, cwd=cwd, environment=environment, timeout=30.0
            )
        except (FileNotFoundError, OSError, subprocess.TimeoutExpired) as error:
            raise ValueError(
                f"prerequisite {expanded.get('id', 'unknown')} failed for "
                f"{bundle.name}: {type(error).__name__}"
            ) from error
        if completed.returncode != 0:
            install = expanded.get("install")
            hint = f"; install with: {' '.join(install)}" if install else ""
            raise ValueError(
                f"prerequisite {expanded.get('id', 'unknown')} failed for {bundle.name}{hint}"
            )
        minimum_text = expanded.get("min_version")
        if isinstance(minimum_text, str):
            actual = _parsed_version(completed.stdout + "\n" + completed.stderr)
            minimum = _parsed_version(minimum_text)
            if actual is None or minimum is None or not _version_at_least(actual, minimum):
                raise ValueError(
                    f"prerequisite {expanded.get('id', 'unknown')} for "
                    f"{bundle.name} requires version {minimum_text} or newer"
                )


def _evaluator_node_options(bundle: Path, platform: str) -> dict[str, Any]:
    descriptor = load_launch_descriptor(bundle, platform)
    evaluate = descriptor.get("semantics", {}).get("evaluate", [])
    if not any(
        isinstance(argument, str) and "${evaluator_node}" in argument for argument in evaluate
    ):
        return {}
    executable = shutil.which("node")
    if executable is None:
        raise ValueError(f"evaluator Node is unavailable for {bundle.name}")
    node_path = Path(executable).resolve()
    try:
        completed = _run_preflight_command(
            [str(node_path), "--version"],
            cwd=bundle,
            environment=os.environ,
            timeout=10.0,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError(f"evaluator Node identity check failed for {bundle.name}") from error
    version = completed.stdout.strip()
    if completed.returncode != 0 or re.fullmatch(r"v\d+\.\d+\.\d+", version) is None:
        raise ValueError(f"evaluator Node version is invalid for {bundle.name}")
    return {
        "evaluator_node_path": node_path,
        "evaluator_node_sha256": digest_file(node_path).removeprefix("sha256:"),
        "evaluator_node_version": version,
    }


def preflight(
    config: ComparisonConfig,
    releases: Sequence[DriverRelease],
    task_manifests: Sequence[Path],
) -> str:
    host = detect_platform()
    if config.platform != host:
        raise ValueError(
            f"selected platform {config.platform} cannot execute on host platform {host}"
        )
    if config.platform == "linux" and not os.environ.get("DISPLAY"):
        raise ValueError("Linux shared-task runs require DISPLAY for X11 or XWayland")
    if not config.codex.is_file():
        raise ValueError(f"Codex CLI is missing: {config.codex}")
    for release in releases:
        _verify_driver_identity(release)
    for manifest in task_manifests:
        _preflight_task(manifest.parent, config.platform)
        _evaluator_node_options(manifest.parent, config.platform)
    version = _preflight_codex(config)
    _codex_provider_environment(config.codex_home)
    return version


def _new_process_group_options(*, detached: bool) -> dict[str, Any]:
    if os.name == "nt":
        flags = subprocess.CREATE_NEW_PROCESS_GROUP
        if detached:
            flags |= subprocess.DETACHED_PROCESS
        return {"creationflags": flags}
    return {"start_new_session": True}


def _terminate_process_group(pid: int, grace_seconds: float = 2.0) -> None:
    if pid <= 0:
        return
    if os.name == "nt":
        try:
            subprocess.run(
                ["taskkill", "/PID", str(pid), "/T", "/F"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
        except OSError:
            pass
        return
    try:
        os.killpg(pid, signal.SIGTERM)
    except OSError:
        return
    deadline = time.monotonic() + grace_seconds
    while time.monotonic() < deadline:
        try:
            os.killpg(pid, 0)
        except OSError:
            return
        time.sleep(0.05)
    try:
        os.killpg(pid, signal.SIGKILL)
    except OSError:
        pass


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, path)


class _XRecordRange8(ctypes.Structure):
    _fields_ = [("first", ctypes.c_ubyte), ("last", ctypes.c_ubyte)]


class _XRecordRange16(ctypes.Structure):
    _fields_ = [("first", ctypes.c_ushort), ("last", ctypes.c_ushort)]


class _XRecordExtRange(ctypes.Structure):
    _fields_ = [
        ("ext_major", _XRecordRange8),
        ("ext_minor", _XRecordRange16),
    ]


class _XRecordRange(ctypes.Structure):
    _fields_ = [
        ("core_requests", _XRecordRange8),
        ("core_replies", _XRecordRange8),
        ("ext_requests", _XRecordExtRange),
        ("ext_replies", _XRecordExtRange),
        ("delivered_events", _XRecordRange8),
        ("device_events", _XRecordRange8),
        ("errors", _XRecordRange8),
        ("client_started", ctypes.c_int),
        ("client_died", ctypes.c_int),
    ]


class _XRecordInterceptData(ctypes.Structure):
    _fields_ = [
        ("id_base", ctypes.c_ulong),
        ("server_time", ctypes.c_ulong),
        ("client_seq", ctypes.c_ulong),
        ("category", ctypes.c_int),
        ("client_swapped", ctypes.c_int),
        ("data", ctypes.POINTER(ctypes.c_ubyte)),
        ("data_len", ctypes.c_ulong),
    ]


class _XWindowAttributes(ctypes.Structure):
    _fields_ = [
        ("x", ctypes.c_int),
        ("y", ctypes.c_int),
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("border_width", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("visual", ctypes.c_void_p),
        ("root", ctypes.c_ulong),
        ("window_class", ctypes.c_int),
        ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int),
        ("backing_store", ctypes.c_int),
        ("backing_planes", ctypes.c_ulong),
        ("backing_pixel", ctypes.c_ulong),
        ("save_under", ctypes.c_int),
        ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int),
        ("map_state", ctypes.c_int),
        ("all_event_masks", ctypes.c_long),
        ("your_event_mask", ctypes.c_long),
        ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int),
        ("screen", ctypes.c_void_p),
    ]


_XRecordCallback = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.POINTER(_XRecordInterceptData))
_XErrorHandler = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)


@_XErrorHandler
def _ignore_x11_error(_display: ctypes.c_void_p, _event: ctypes.c_void_p) -> int:
    return 0


class _X11SampleSource:
    def __init__(self, display_name: str) -> None:
        library_path = ctypes.util.find_library("X11")
        if not library_path:
            raise RuntimeError("libX11 is unavailable")
        self.library = ctypes.CDLL(library_path)
        self._configure_x11()
        self.library.XInitThreads()
        self.library.XSetErrorHandler(_ignore_x11_error)
        encoded_display = display_name.encode("utf-8")
        self.display = self.library.XOpenDisplay(encoded_display)
        if not self.display:
            raise RuntimeError(f"could not open X11 display {display_name!r}")
        self.root_window = self.library.XDefaultRootWindow(self.display)
        self.record_library: Any | None = None
        self.record_display: Any | None = None
        self.record_context = 0
        self.record_range: Any | None = None
        self.record_callback: Any | None = None
        self.recorded_events: list[ForegroundEvent] = []
        self.drag_events_available = False
        self.drag_error: str | None = None
        try:
            self._start_recording(encoded_display)
        except Exception as error:  # noqa: BLE001 - partial diagnostics remain useful
            self.drag_error = f"{type(error).__name__}: {error}"
            self._close_recording()

    def _configure_x11(self) -> None:
        self.library.XInitThreads.restype = ctypes.c_int
        self.library.XSetErrorHandler.argtypes = [_XErrorHandler]
        self.library.XSetErrorHandler.restype = ctypes.c_void_p
        self.library.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.library.XOpenDisplay.restype = ctypes.c_void_p
        self.library.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        self.library.XDefaultRootWindow.restype = ctypes.c_ulong
        self.library.XGetInputFocus.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.c_int),
        ]
        self.library.XGetInputFocus.restype = ctypes.c_int
        self.library.XQueryPointer.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_uint),
        ]
        self.library.XQueryPointer.restype = ctypes.c_int
        self.library.XGetWindowAttributes.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.POINTER(_XWindowAttributes),
        ]
        self.library.XGetWindowAttributes.restype = ctypes.c_int
        self.library.XFlush.argtypes = [ctypes.c_void_p]
        self.library.XFlush.restype = ctypes.c_int
        self.library.XFree.argtypes = [ctypes.c_void_p]
        self.library.XFree.restype = ctypes.c_int
        self.library.XCloseDisplay.argtypes = [ctypes.c_void_p]
        self.library.XCloseDisplay.restype = ctypes.c_int

    def _start_recording(self, display_name: bytes) -> None:
        record_path = ctypes.util.find_library("Xtst")
        if not record_path:
            raise RuntimeError("libXtst with XRecord support is unavailable")
        self.record_library = ctypes.CDLL(record_path)
        self._configure_record_library()
        self.record_display = self.library.XOpenDisplay(display_name)
        if not self.record_display:
            raise RuntimeError("could not open the XRecord display connection")
        major = ctypes.c_int()
        minor = ctypes.c_int()
        if not self.record_library.XRecordQueryVersion(
            self.record_display, ctypes.byref(major), ctypes.byref(minor)
        ):
            raise RuntimeError("XRecord is unavailable on the X11 display")
        self.record_range = self.record_library.XRecordAllocRange()
        if not self.record_range:
            raise RuntimeError("XRecordAllocRange failed")
        self.record_range.contents.core_requests = _XRecordRange8(26, 27)
        self.record_range.contents.delivered_events = _XRecordRange8(9, 18)
        self.record_range.contents.device_events = _XRecordRange8(4, 5)
        clients = (ctypes.c_ulong * 1)(_XRECORD_ALL_CLIENTS)
        ranges = (ctypes.POINTER(_XRecordRange) * 1)(self.record_range)
        self.record_context = int(
            self.record_library.XRecordCreateContext(self.record_display, 0, clients, 1, ranges, 1)
        )
        if not self.record_context:
            raise RuntimeError("XRecordCreateContext failed")
        self.record_callback = _XRecordCallback(self._record_intercept)
        if not self.record_library.XRecordEnableContextAsync(
            self.record_display,
            self.record_context,
            self.record_callback,
            None,
        ):
            raise RuntimeError("XRecordEnableContextAsync failed")
        self.record_library.XRecordProcessReplies(self.record_display)
        self.drag_events_available = True

    def _configure_record_library(self) -> None:
        assert self.record_library is not None
        self.record_library.XRecordQueryVersion.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_int),
        ]
        self.record_library.XRecordQueryVersion.restype = ctypes.c_int
        self.record_library.XRecordAllocRange.restype = ctypes.POINTER(_XRecordRange)
        self.record_library.XRecordCreateContext.argtypes = [
            ctypes.c_void_p,
            ctypes.c_int,
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.c_int,
            ctypes.POINTER(ctypes.POINTER(_XRecordRange)),
            ctypes.c_int,
        ]
        self.record_library.XRecordCreateContext.restype = ctypes.c_ulong
        self.record_library.XRecordEnableContextAsync.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            _XRecordCallback,
            ctypes.c_void_p,
        ]
        self.record_library.XRecordEnableContextAsync.restype = ctypes.c_int
        self.record_library.XRecordProcessReplies.argtypes = [ctypes.c_void_p]
        self.record_library.XRecordDisableContext.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        self.record_library.XRecordDisableContext.restype = ctypes.c_int
        self.record_library.XRecordFreeContext.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        self.record_library.XRecordFreeContext.restype = ctypes.c_int
        self.record_library.XRecordFreeData.argtypes = [ctypes.POINTER(_XRecordInterceptData)]

    def _record_intercept(
        self,
        _closure: ctypes.c_void_p,
        pointer: ctypes.POINTER(_XRecordInterceptData),
    ) -> None:
        assert self.record_library is not None
        try:
            data = pointer.contents
            raw = (
                ctypes.string_at(data.data, data.data_len * 4)
                if data.data and data.data_len
                else b""
            )
            if data.category == _XRECORD_FROM_SERVER:
                self._decode_server_events(data, raw)
            elif data.category == _XRECORD_FROM_CLIENT:
                self._decode_client_requests(data, raw)
        finally:
            self.record_library.XRecordFreeData(pointer)

    def _event(
        self,
        data: _XRecordInterceptData,
        kind: str,
        *,
        window: int | None = None,
        button: int | None = None,
    ) -> None:
        self.recorded_events.append(
            ForegroundEvent(
                kind=kind,
                observed_at=datetime.now(UTC).isoformat(),
                monotonic_ns=time.monotonic_ns(),
                server_time_ms=int(data.server_time),
                client_id=int(data.id_base),
                window=window,
                button=button,
            )
        )

    def _decode_server_events(self, data: _XRecordInterceptData, raw: bytes) -> None:
        for offset in range(0, len(raw) - 31, 32):
            event = raw[offset : offset + 32]
            event_type = event[0] & 0x7F
            if event_type in {4, 5}:
                self._event(
                    data,
                    "button_press" if event_type == 4 else "button_release",
                    window=int.from_bytes(event[12:16], sys.byteorder),
                    button=int(event[1]),
                )
            elif event_type in {9, 10}:
                self._event(
                    data,
                    "focus_in" if event_type == 9 else "focus_out",
                    window=int.from_bytes(event[4:8], sys.byteorder),
                )
            elif event_type in {17, 18}:
                self._event(
                    data,
                    "window_destroy" if event_type == 17 else "window_unmap",
                    window=int.from_bytes(event[8:12], sys.byteorder),
                )

    def _decode_client_requests(self, data: _XRecordInterceptData, raw: bytes) -> None:
        offset = 0
        while offset + 4 <= len(raw):
            request = raw[offset:]
            opcode = request[0]
            length = int.from_bytes(request[2:4], sys.byteorder) * 4
            if length <= 0 or offset + length > len(raw):
                break
            if opcode == 26:
                self._event(
                    data,
                    "pointer_grab",
                    window=int.from_bytes(request[4:8], sys.byteorder),
                )
            elif opcode == 27:
                self._event(data, "pointer_ungrab")
            offset += length

    def _window_viewable(self, window: int) -> bool:
        if window in {_X11_NONE, _X11_POINTER_ROOT}:
            return False
        attributes = _XWindowAttributes()
        return bool(
            self.library.XGetWindowAttributes(self.display, window, ctypes.byref(attributes))
            and attributes.map_state == _X11_IS_VIEWABLE
        )

    def sample(self) -> ForegroundSample:
        if self.record_display and self.record_library and self.record_context:
            self.record_library.XRecordProcessReplies(self.record_display)
        focus_window = ctypes.c_ulong()
        revert_to = ctypes.c_int()
        if not self.library.XGetInputFocus(
            self.display, ctypes.byref(focus_window), ctypes.byref(revert_to)
        ):
            raise RuntimeError("XGetInputFocus failed")
        root_return = ctypes.c_ulong()
        child_return = ctypes.c_ulong()
        root_x = ctypes.c_int()
        root_y = ctypes.c_int()
        window_x = ctypes.c_int()
        window_y = ctypes.c_int()
        button_mask = ctypes.c_uint()
        if not self.library.XQueryPointer(
            self.display,
            self.root_window,
            ctypes.byref(root_return),
            ctypes.byref(child_return),
            ctypes.byref(root_x),
            ctypes.byref(root_y),
            ctypes.byref(window_x),
            ctypes.byref(window_y),
            ctypes.byref(button_mask),
        ):
            raise RuntimeError("XQueryPointer failed")
        events = tuple(self.recorded_events)
        self.recorded_events.clear()
        focus = int(focus_window.value)
        pointer_window = int(child_return.value)
        return ForegroundSample(
            focus_window=focus,
            cursor_x=int(root_x.value),
            cursor_y=int(root_y.value),
            button_mask=int(button_mask.value) & _X11_BUTTON_MASK,
            focus_window_viewable=self._window_viewable(focus),
            pointer_window=pointer_window,
            pointer_window_viewable=self._window_viewable(pointer_window),
            observed_at=datetime.now(UTC).isoformat(),
            monotonic_ns=time.monotonic_ns(),
            events=events,
        )

    def _close_recording(self) -> None:
        if self.record_library is not None and self.record_context:
            try:
                if self.display:
                    self.record_library.XRecordDisableContext(self.display, self.record_context)
                    self.library.XFlush(self.display)
                if self.record_display:
                    self.record_library.XRecordProcessReplies(self.record_display)
                    self.record_library.XRecordFreeContext(self.record_display, self.record_context)
            except Exception:
                pass
            self.record_context = 0
        if self.record_range is not None:
            self.library.XFree(ctypes.cast(self.record_range, ctypes.c_void_p))
            self.record_range = None
        if self.record_display:
            self.library.XCloseDisplay(self.record_display)
            self.record_display = None

    def close(self) -> None:
        self._close_recording()
        if self.display:
            self.library.XCloseDisplay(self.display)
            self.display = None


class ForegroundDisturbanceObserver:
    def __init__(
        self,
        gui_environment: Mapping[str, str],
        source_factory: Callable[[str], Any] | None = None,
        sample_hz: int = FOREGROUND_SAMPLE_HZ,
    ) -> None:
        self.gui_environment = dict(gui_environment)
        self.source_factory = source_factory or _X11SampleSource
        self.sample_hz = sample_hz
        self.counter = _ForegroundDisturbanceCounter(CURSOR_DEVIATION_THRESHOLD_PX)
        self.source: Any | None = None
        self.thread: threading.Thread | None = None
        self.stop_event = threading.Event()
        self.summary_path: Path | None = None
        self.evidence_path: Path | None = None
        self.evidence_handle: Any | None = None
        self.started_at: str | None = None
        self.started_monotonic: float | None = None
        self.sampling_error: str | None = None
        self.evidence_error: str | None = None
        self.available = False
        self.finished = False

    def start(self, context: TrialContext) -> None:
        observer_dir = context.trial_dir / "observer"
        self.summary_path = observer_dir / "foreground-disturbance.json"
        self.evidence_path = observer_dir / "foreground-disturbance.ndjson"
        self.started_at = datetime.now(UTC).isoformat()
        self.started_monotonic = time.monotonic()
        display_name = self.gui_environment.get("DISPLAY")
        if detect_platform() != "linux":
            self.sampling_error = "foreground disturbance sampling requires Linux"
            return
        if not display_name:
            self.sampling_error = "DISPLAY is not set"
            return
        try:
            observer_dir.mkdir(parents=True, exist_ok=True)
            self.evidence_handle = self.evidence_path.open("x", encoding="utf-8", buffering=1)
        except Exception as error:  # noqa: BLE001 - diagnostics remain optional
            self.evidence_error = f"{type(error).__name__}: {error}"
        try:
            self.source = self.source_factory(display_name)
            self._capture_sample()
        except Exception as error:  # noqa: BLE001 - diagnostic evidence is optional
            self.sampling_error = f"{type(error).__name__}: {error}"
            return
        self.thread = threading.Thread(
            target=self._sample_loop,
            name="foreground-disturbance-observer",
            daemon=True,
        )
        self.thread.start()

    def _write_evidence(self, value: Mapping[str, Any]) -> None:
        if self.evidence_handle is None:
            return
        try:
            self.evidence_handle.write(json.dumps(value, sort_keys=True) + "\n")
        except Exception as error:  # noqa: BLE001 - sampling should continue
            self.evidence_error = f"{type(error).__name__}: {error}"
            try:
                self.evidence_handle.close()
            except Exception:
                pass
            self.evidence_handle = None

    def _capture_sample(self) -> None:
        sample = self.source.sample()
        transitions = self.counter.add(sample)
        self._write_evidence(
            {
                "type": "sample",
                "observed_at": sample.observed_at or datetime.now(UTC).isoformat(),
                "monotonic_ns": sample.monotonic_ns or time.monotonic_ns(),
                "focus_window": sample.focus_window,
                "focus_window_viewable": sample.focus_window_viewable,
                "pointer_window": sample.pointer_window,
                "pointer_window_viewable": sample.pointer_window_viewable,
                "cursor_x": sample.cursor_x,
                "cursor_y": sample.cursor_y,
                "button_mask": sample.button_mask,
                "events": [asdict(event) for event in sample.events],
                "transitions": transitions,
            }
        )

    def _sample_loop(self) -> None:
        interval_seconds = 1 / self.sample_hz
        while not self.stop_event.wait(interval_seconds):
            try:
                self._capture_sample()
            except Exception as error:  # noqa: BLE001 - diagnostic evidence is optional
                self.sampling_error = f"{type(error).__name__}: {error}"
                self.stop_event.set()
                return

    def finish(self) -> None:
        if self.finished:
            return
        self.finished = True
        self.stop_event.set()
        if self.thread is not None:
            self.thread.join(timeout=max(1.0, 2 / self.sample_hz))
            if self.thread.is_alive() and self.sampling_error is None:
                self.sampling_error = "sampling thread did not stop"
        teardown_transitions = self.counter.finish()
        if teardown_transitions:
            self._write_evidence(
                {
                    "type": "observer_teardown",
                    "observed_at": datetime.now(UTC).isoformat(),
                    "monotonic_ns": time.monotonic_ns(),
                    "transitions": teardown_transitions,
                }
            )
        if self.source is not None:
            try:
                self.source.close()
            except Exception:
                pass
        if self.evidence_handle is not None:
            try:
                self.evidence_handle.close()
            except Exception as error:  # noqa: BLE001 - include evidence failure
                if self.evidence_error is None:
                    self.evidence_error = f"{type(error).__name__}: {error}"
            self.evidence_handle = None
        sample_available = self.sampling_error is None and self.counter.sample_count > 0
        evidence_available = self.evidence_error is None and self.evidence_path is not None
        focus_available = sample_available and evidence_available
        cursor_available = sample_available and evidence_available
        source_drag_available = bool(
            self.source is not None and getattr(self.source, "drag_events_available", True)
        )
        drag_available = sample_available and evidence_available and source_drag_available
        base_error = self.sampling_error or self.evidence_error
        focus_error = None if focus_available else base_error or "focus samples unavailable"
        cursor_error = None if cursor_available else base_error or "cursor samples unavailable"
        drag_error = None
        if not drag_available:
            drag_error = (
                base_error
                or getattr(self.source, "drag_error", None)
                or "XRecord drag events unavailable"
            )
        focus_drops = self.counter.foreground_keyboard_focus_drops if focus_available else None
        drag_interruptions = self.counter.foreground_drag_interruptions if drag_available else None
        drag_observations = self.counter.foreground_drag_observations if drag_available else None
        cursor_deviations = (
            self.counter.foreground_cursor_trajectory_deviations if cursor_available else None
        )
        foreground_disturbances = (
            focus_drops + drag_interruptions + cursor_deviations
            if focus_drops is not None
            and drag_interruptions is not None
            and cursor_deviations is not None
            else None
        )
        self.available = foreground_disturbances is not None
        finished_monotonic = time.monotonic()
        duration_ms = (
            int((finished_monotonic - self.started_monotonic) * 1000)
            if self.started_monotonic is not None
            else None
        )
        if self.summary_path is not None:
            _write_json(
                self.summary_path,
                {
                    "schema_version": "2",
                    "available": self.available,
                    "measurement": FOREGROUND_DISTURBANCE_MEASUREMENT,
                    "sample_hz": self.sample_hz,
                    "sample_count": self.counter.sample_count,
                    "started_at": self.started_at,
                    "finished_at": datetime.now(UTC).isoformat(),
                    "duration_ms": duration_ms,
                    "evidence_path": self.evidence_path.name if self.evidence_path else None,
                    "foreground_keyboard_focus_available": focus_available,
                    "foreground_keyboard_focus_error": focus_error,
                    "foreground_keyboard_focus_drops": focus_drops,
                    "foreground_drag_available": drag_available,
                    "foreground_drag_error": drag_error,
                    "foreground_drag_interruptions": drag_interruptions,
                    "foreground_drag_observations": drag_observations,
                    "foreground_cursor_trajectory_available": cursor_available,
                    "foreground_cursor_trajectory_error": cursor_error,
                    "foreground_cursor_trajectory_deviations": cursor_deviations,
                    "cursor_deviation_threshold_px": CURSOR_DEVIATION_THRESHOLD_PX,
                    "foreground_disturbances": foreground_disturbances,
                    "focus_window_transitions": (
                        self.counter.focus_window_transitions if sample_available else None
                    ),
                    "cursor_deviation_episodes": cursor_deviations,
                    "error": (
                        None
                        if self.available
                        else base_error or drag_error or "one or more components unavailable"
                    ),
                },
            )


class DriverDaemon:
    def __init__(
        self,
        release: DriverRelease,
        log_root: Path,
        gui_environment: Mapping[str, str],
    ) -> None:
        self.release = release
        self.log_root = log_root
        self.gui_environment = dict(gui_environment)
        self.process: subprocess.Popen[bytes] | None = None
        self.stdout_handle: Any = None
        self.stderr_handle: Any = None
        self.socket_directory: Path | None = None
        if os.name == "nt":
            self.endpoint = rf"\\.\pipe\cdb-{uuid.uuid4().hex}"
        else:
            self.socket_directory = Path(tempfile.mkdtemp(prefix="cdb-cua-"))
            self.endpoint = str(self.socket_directory / "driver.sock")

    @property
    def stdout_path(self) -> Path:
        return self.log_root.with_suffix(".stdout")

    @property
    def stderr_path(self) -> Path:
        return self.log_root.with_suffix(".stderr")

    def __enter__(self) -> DriverDaemon:
        self.log_root.parent.mkdir(parents=True, exist_ok=True)
        self.stdout_handle = self.stdout_path.open("wb")
        self.stderr_handle = self.stderr_path.open("wb")
        environment = clean_environment(
            {
                **self.gui_environment,
                "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
            }
        )
        try:
            self.process = subprocess.Popen(
                [
                    str(self.release.binary),
                    "serve",
                    "--socket",
                    self.endpoint,
                    "--dangerously-bypass-approvals",
                ],
                cwd=self.release.root,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=self.stdout_handle,
                stderr=self.stderr_handle,
                shell=False,
                **_new_process_group_options(detached=False),
            )
        except BaseException:
            self.close()
            raise
        deadline = time.monotonic() + 20.0
        while time.monotonic() < deadline:
            try:
                completed = subprocess.run(
                    [
                        str(self.release.binary),
                        "status",
                        "--socket",
                        self.endpoint,
                    ],
                    cwd=self.release.root,
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=3.0,
                    check=False,
                    shell=False,
                )
                if completed.returncode == 0:
                    return self
            except (OSError, subprocess.TimeoutExpired):
                pass
            time.sleep(0.2)
        self.close()
        raise HarnessFailure(f"Cua Driver {self.release.version} daemon did not become ready")

    def close(self) -> None:
        environment = clean_environment(
            {
                **self.gui_environment,
                "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
            }
        )
        try:
            subprocess.run(
                [
                    str(self.release.binary),
                    "stop",
                    "--socket",
                    self.endpoint,
                ],
                cwd=self.release.root,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=5.0,
                check=False,
                shell=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass
        if self.process is not None and self.process.poll() is None:
            try:
                self.process.wait(timeout=3.0)
            except subprocess.TimeoutExpired:
                _terminate_process_group(self.process.pid)
        for handle in (self.stdout_handle, self.stderr_handle):
            if handle is not None and not handle.closed:
                handle.close()
        if self.socket_directory is not None:
            shutil.rmtree(self.socket_directory, ignore_errors=True)

    def __exit__(self, _type: Any, _value: Any, _traceback: Any) -> None:
        self.close()


class _SocketMcpClient(McpClient):
    def __init__(
        self,
        command: Path,
        endpoint: str,
        gui_environment: Mapping[str, str],
        stderr_path: Path,
    ) -> None:
        self.stderr_handle = stderr_path.open("ab")
        try:
            self.process = subprocess.Popen(
                [str(command), "mcp", "--socket", endpoint],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=self.stderr_handle,
                text=True,
                encoding="utf-8",
                env=clean_environment(
                    {
                        **gui_environment,
                        "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
                    }
                ),
                shell=False,
                **_new_process_group_options(detached=False),
            )
        except BaseException:
            self.stderr_handle.close()
            raise
        if self.process.stdin is None or self.process.stdout is None:
            raise HarnessFailure("could not open Cua Driver MCP pipes")
        self.stdin = self.process.stdin
        self.responses: queue.Queue[str] = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.next_id = 2
        initialized = self._request(1, "initialize", {})
        if "error" in initialized:
            self.close()
            raise HarnessFailure("Cua Driver MCP initialize failed")
        self.server_version = initialized.get("result", {}).get("serverInfo", {}).get("version")

    def close(self) -> None:
        try:
            super().close()
        finally:
            if not self.stderr_handle.closed:
                self.stderr_handle.close()


class SocketCuaRecordingObserver(CuaRecordingObserver):
    def __init__(
        self,
        command: Path,
        endpoint: str,
        expected_version: str,
        gui_environment: Mapping[str, str],
    ) -> None:
        super().__init__()
        self.command = command
        self.endpoint = endpoint
        self.expected_version = expected_version
        self.gui_environment = dict(gui_environment)

    def start(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        requirements: tuple[Mapping[str, Any], ...],
    ) -> None:
        platform = handle.facts.get("platform")
        self.platform = platform if platform in {"macos", "windows", "linux"} else detect_platform()
        observer_root = context.trial_dir / "observer"
        observer_root.mkdir(exist_ok=True)
        self.recording_dir = observer_root / "cua-driver-recording"
        self.recording_dir.mkdir()
        self.requirements = requirements
        self.client = _SocketMcpClient(
            self.command,
            self.endpoint,
            self.gui_environment,
            context.artifacts / "observer-mcp.stderr",
        )
        self.provider_version = self.client.server_version
        if self.provider_version != self.expected_version:
            self.client.close()
            self.client = None
            raise HarnessFailure(
                f"observer expected Cua Driver {self.expected_version}, got "
                f"{self.provider_version!r}"
            )
        self.initial_apps = self._apps_by_pid()
        self.client.call(
            "start_recording",
            {"output_dir": str(self.recording_dir), "record_video": False},
        )

    def finish(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome | None,
    ) -> ObserverReport:
        del context, handle, outcome
        if self.client is None:
            return ObserverReport(
                name=self.name,
                trust="unavailable",
                detail="Cua Driver observer did not start",
            )
        inventory_available = True
        try:
            try:
                apps = merge_app_maps(self.initial_apps, self._apps_by_pid())
            except HarnessFailure:
                inventory_available = False
                apps = dict(self.initial_apps)
            try:
                self.client.call("stop_recording", {})
            except HarnessFailure:
                pass
            events = tuple(self._events(apps))
            detail = f"mapped {len(events)} normalized events"
            if not inventory_available:
                detail += "; final app inventory unavailable after MCP disconnect"
            return ObserverReport(
                name=self.name,
                trust=self.trust,
                events=events,
                detail=detail,
            )
        finally:
            self.client.close()
            self.client = None


class RecordingHarness:
    name = "subprocess"

    def __init__(
        self,
        delegate: Any,
        observer: SocketCuaRecordingObserver,
        disturbance_observer: ForegroundDisturbanceObserver,
    ) -> None:
        self.delegate = delegate
        self.observer = observer
        self.disturbance_observer = disturbance_observer

    def run(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        interrupt: InterruptFlag,
        timeout_seconds: float,
    ) -> AgentOutcome:
        owns_recording = self.observer.client is None
        if owns_recording:
            try:
                self.observer.start(context, handle, ())
                context.emit(
                    "diagnostic_recording_started",
                    {"observer": self.observer.name},
                )
            except Exception as error:  # noqa: BLE001 - grading remains independent
                owns_recording = False
                context.emit(
                    "participation_observer_error",
                    {"phase": "diagnostic_start", "error_type": type(error).__name__},
                )
        self.disturbance_observer.start(context)
        try:
            return self.delegate.run(context, handle, interrupt, timeout_seconds)
        finally:
            try:
                self.disturbance_observer.finish()
                context.emit(
                    "foreground_disturbance_observer_finished",
                    {"available": self.disturbance_observer.available},
                )
            except Exception as error:  # noqa: BLE001 - grading remains independent
                context.emit(
                    "participation_observer_error",
                    {
                        "phase": "foreground_disturbance_finish",
                        "error_type": type(error).__name__,
                    },
                )
            if owns_recording:
                try:
                    report = self.observer.finish(context, handle, None)
                    context.emit(
                        "diagnostic_recording_finished",
                        {"observer": report.name, "detail": report.detail},
                    )
                except Exception as error:  # noqa: BLE001 - grading remains independent
                    context.emit(
                        "participation_observer_error",
                        {
                            "phase": "diagnostic_finish",
                            "error_type": type(error).__name__,
                        },
                    )


class EnvironmentSubprocessHarness:
    name = "subprocess"

    def __init__(self, delegate: Any, environment: Mapping[str, str]) -> None:
        self.command = delegate.command
        self.expected_digest = delegate.expected_digest
        self.environment = dict(environment)

    def run(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        interrupt: InterruptFlag,
        timeout_seconds: float,
    ) -> AgentOutcome:
        if digest_file(self.command) != self.expected_digest:
            raise ValidationFailure("materialized agent command digest changed")
        stdout = context.artifacts / "agent.stdout"
        stderr = context.artifacts / "agent.stderr"
        argv = command_for(
            self.command,
            ["--workspace", str(handle.root), "--artifacts", str(context.artifacts)],
        )
        context.emit("process_spawned", {"role": "agent", "argv0": self.command.name})
        try:
            process = run_process(
                argv,
                cwd=context.harness_workspace,
                stdout_path=stdout,
                stderr_path=stderr,
                timeout_seconds=timeout_seconds,
                interrupt=interrupt,
                extra_env=self.environment,
            )
        except (DeadlineExceeded, TrialInterrupted, HardAbort) as error:
            context.emit(
                "process_exited",
                {"role": "agent", "exit_code": None, "reason": error.status},
            )
            raise
        context.emit(
            "process_exited",
            {
                "role": "agent",
                "exit_code": process.returncode,
                "truncated": process.truncated,
                "output_exceeded": process.output_exceeded,
            },
        )
        outputs = tuple(
            path.relative_to(context.artifacts).as_posix()
            for path in sorted(
                context.artifacts.rglob("*"),
                key=lambda candidate: candidate.relative_to(context.artifacts).as_posix(),
            )
            if path.is_file()
        )
        return AgentOutcome(
            completed=process.returncode == 0,
            exit_code=process.returncode,
            duration_ms=process.duration_ms,
            artifacts=outputs,
        )


@contextmanager
def _local_adapter_override(
    release: DriverRelease,
    endpoint: str,
    gui_environment: Mapping[str, str],
    agent_environment: Mapping[str, str],
):
    original = engine.adapters

    def factory(*args: Any, **kwargs: Any) -> tuple[Any, Any, Any, Any]:
        environment, agent, _observer, evaluator = local_adapters(*args, **kwargs)
        observer = SocketCuaRecordingObserver(
            release.binary,
            endpoint,
            release.version,
            gui_environment,
        )
        forwarding_agent = EnvironmentSubprocessHarness(agent, agent_environment)
        disturbance_observer = ForegroundDisturbanceObserver(gui_environment)
        return (
            environment,
            RecordingHarness(forwarding_agent, observer, disturbance_observer),
            observer,
            evaluator,
        )

    engine.adapters = factory
    try:
        yield
    finally:
        engine.adapters = original


def _safe_skill_member(name: str) -> PurePosixPath | None:
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts or path.as_posix() == ".":
        return None
    return path


def _load_skill_directory(root: Path) -> tuple[BundledSkillFile, ...]:
    files: list[BundledSkillFile] = []
    for path in sorted(root.rglob("*"), key=lambda item: item.as_posix()):
        if path.is_file() and not path.is_symlink():
            files.append(
                BundledSkillFile(
                    PurePosixPath(path.relative_to(root).as_posix()), path.read_bytes()
                )
            )
    return tuple(files)


def _load_skill_archive(path: Path) -> tuple[BundledSkillFile, ...]:
    with tarfile.open(path, mode="r:*") as archive:
        members = [
            member
            for member in archive.getmembers()
            if member.isfile() and _safe_skill_member(member.name) is not None
        ]
        skill_markers = [
            PurePosixPath(member.name)
            for member in members
            if PurePosixPath(member.name).name == "SKILL.md"
            and "cua-driver" in PurePosixPath(member.name).parts
        ]
        if not skill_markers:
            return ()
        root = skill_markers[0].parent
        files: list[BundledSkillFile] = []
        for member in sorted(members, key=lambda item: item.name):
            member_path = PurePosixPath(member.name)
            try:
                relative = member_path.relative_to(root)
            except ValueError:
                continue
            extracted = archive.extractfile(member)
            if extracted is None:
                continue
            files.append(BundledSkillFile(relative, extracted.read()))
        return tuple(files)


def _load_skill(config: Mapping[str, Any]) -> tuple[BundledSkillFile, ...]:
    source = config.get("skill_source")
    kind = config.get("skill_kind")
    if not isinstance(source, str) or not source:
        return ()
    path = Path(source)
    if kind == "directory":
        return _load_skill_directory(path)
    if kind == "archive":
        return _load_skill_archive(path)
    return ()


def _codex_runtime_root() -> Path:
    configured = os.environ.get("XDG_CACHE_HOME")
    cache_home = Path(configured).expanduser() if configured else Path.home() / ".cache"
    root = (cache_home / "cua-driver-bench" / "codex").resolve()
    root.mkdir(parents=True, exist_ok=True)
    return root


def _link_auth(source_home: Path, isolated_codex_home: Path) -> None:
    source = source_home / "auth.json"
    if not source.is_file():
        return
    destination = isolated_codex_home / "auth.json"
    try:
        destination.symlink_to(source)
    except OSError:
        try:
            os.link(source, destination)
        except OSError as error:
            raise RuntimeError(
                "could not link Codex auth.json into the isolated CODEX_HOME"
            ) from error


def _toml_key(value: str) -> str:
    return json.dumps(value, ensure_ascii=True)


def _toml_value(value: Any) -> str:
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return repr(value)
    if isinstance(value, list):
        return "[" + ", ".join(_toml_value(item) for item in value) + "]"
    raise ValueError(f"unsupported Codex provider config value: {type(value).__name__}")


def _render_toml_table(path: tuple[str, ...], values: Mapping[str, Any]) -> list[str]:
    lines = ["[" + ".".join(_toml_key(part) for part in path) + "]"]
    nested: list[tuple[str, Mapping[str, Any]]] = []
    for key, value in values.items():
        if isinstance(value, Mapping):
            nested.append((str(key), value))
        else:
            lines.append(f"{_toml_key(str(key))} = {_toml_value(value)}")
    for key, table in nested:
        lines.append("")
        lines.extend(_render_toml_table((*path, key), table))
    return lines


def _codex_provider_config(codex_home: Path) -> tuple[list[str], str | None]:
    path = codex_home / "config.toml"
    if not path.is_file():
        return [], None
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ValueError(f"Codex config is invalid: {path}") from error
    provider_name = document.get("model_provider")
    if provider_name is None:
        return [], None
    if not isinstance(provider_name, str) or not provider_name:
        raise ValueError("Codex model_provider must be a non-empty string")
    providers = document.get("model_providers")
    provider = providers.get(provider_name) if isinstance(providers, Mapping) else None
    if not isinstance(provider, Mapping):
        raise ValueError(f"Codex model provider is not defined: {provider_name}")
    environment_key = provider.get("env_key")
    if environment_key is not None and (
        not isinstance(environment_key, str)
        or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", environment_key) is None
    ):
        raise ValueError("Codex model provider env_key is invalid")
    lines = [f"model_provider = {_toml_value(provider_name)}"]
    lines.extend(_render_toml_table(("model_providers", provider_name), provider))
    return lines, environment_key


def _codex_provider_environment(codex_home: Path) -> dict[str, str]:
    _provider_lines, environment_key = _codex_provider_config(codex_home)
    if environment_key is None:
        return {}
    environment_value = os.environ.get(environment_key)
    if environment_value is None:
        raise RuntimeError("Codex model provider requires environment variable " + environment_key)
    return {environment_key: environment_value}


def _codex_telemetry(
    events_path: Path, model_route: ModelRoute
) -> tuple[dict[str, Any], str | None]:
    harness = production_harness("codex")
    normalized: list[dict[str, Any]] = []
    terminal_failure: str | None = None
    usage: dict[str, int] | None = None
    try:
        lines = events_path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError):
        lines = []
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict):
            continue
        item = harness.normalize_telemetry(event, model_route)
        document = asdict(item)
        normalized.append(document)
        if item.terminal_failure is not None:
            terminal_failure = item.terminal_failure
        if item.usage_is_cumulative_total:
            candidate = {
                key: value
                for key, value in {
                    "input_tokens": item.input_tokens,
                    "output_tokens": item.output_tokens,
                    "cache_read_tokens": item.cache_read_tokens,
                    "cache_write_tokens": item.cache_write_tokens,
                }.items()
                if value is not None
            }
            if candidate:
                usage = candidate
    return {"events": normalized, "usage": usage}, terminal_failure


def _codex_turn_completed(events_path: Path) -> bool:
    try:
        with events_path.open("rb") as events:
            events.seek(0, os.SEEK_END)
            events.seek(max(0, events.tell() - 65_536))
            lines = events.read().splitlines()
    except OSError:
        return False
    for line in reversed(lines):
        try:
            event = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        return event.get("type") == "turn.completed"
    return False


def _wait_for_codex(
    process: subprocess.Popen[bytes],
    events_path: Path,
    timeout_seconds: float,
    terminal_grace_seconds: float = 5.0,
) -> tuple[int, bool, bool]:
    deadline = time.monotonic() + max(1.0, timeout_seconds)
    terminal_observed_at: float | None = None
    while process.poll() is None:
        now = time.monotonic()
        if _codex_turn_completed(events_path):
            if terminal_observed_at is None:
                terminal_observed_at = now
            if now - terminal_observed_at >= terminal_grace_seconds:
                _terminate_process_group(process.pid)
                return 0, False, True
        if now >= deadline:
            _terminate_process_group(process.pid)
            return 124, True, False
        time.sleep(0.2)
    return int(process.returncode), False, False


def _launch_apps(
    descriptor: Mapping[str, Any],
    variables: Mapping[str, str],
    artifacts: Path,
    gui_environment: Mapping[str, str],
) -> list[dict[str, Any]]:
    processes: list[dict[str, Any]] = []
    process_path = artifacts / "app-processes.json"
    for raw_app in descriptor["apps"]:
        if not isinstance(raw_app, dict):
            raise ValueError("launch descriptor app must be an object")
        app = expand_placeholders(raw_app, variables)
        app_id = str(app.get("id", "unknown"))
        optional = app.get("optional") is True
        environment = {**os.environ, **gui_environment}
        environment.update(
            {str(key): str(value) for key, value in dict(app.get("env", {})).items()}
        )
        for name in app.get("unset_env", []):
            environment.pop(str(name), None)
        cwd = Path(app.get("cwd", variables["bundle"])).resolve()
        try:
            command = _resolve_executable(app["command"])
        except (KeyError, FileNotFoundError, ValueError):
            if optional:
                processes.append({"app_id": app_id, "kind": app.get("kind"), "skipped": True})
                _write_json(process_path, processes)
                continue
            raise
        log_root = artifacts / "apps" / app_id
        log_root.parent.mkdir(parents=True, exist_ok=True)
        stdout = log_root.with_suffix(".stdout").open("wb")
        stderr = log_root.with_suffix(".stderr").open("wb")
        try:
            process = subprocess.Popen(
                command,
                cwd=cwd,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                shell=False,
                **_new_process_group_options(detached=True),
            )
        finally:
            stdout.close()
            stderr.close()
        record = {
            "app_id": app_id,
            "kind": app.get("kind"),
            "pid": process.pid,
            "process_group": process.pid,
            "skipped": False,
        }
        processes.append(record)
        _write_json(process_path, processes)
        ready = app.get("ready")
        if isinstance(ready, dict) and isinstance(ready.get("url"), str):
            deadline = time.monotonic() + float(ready.get("timeout_seconds", 15))
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"app {app_id} exited before readiness")
                try:
                    with urllib.request.urlopen(ready["url"], timeout=1.0) as response:
                        if response.status < 500:
                            break
                except (OSError, urllib.error.URLError):
                    time.sleep(0.2)
            else:
                raise TimeoutError(f"app {app_id} readiness timed out")
    time.sleep(1.0)
    return processes


def _run_codex(
    config: Mapping[str, Any],
    workspace: Path,
    artifacts: Path,
    bundle: Path,
    gui_environment: Mapping[str, str],
) -> int:
    brief = bundle / str(config["brief"])
    if not brief.is_file():
        raise FileNotFoundError(f"participant brief is missing: {brief}")
    events_path = artifacts / "codex-events.jsonl"
    stderr_path = artifacts / "codex.stderr"
    run_path = artifacts / "codex-run.json"
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="cdb-codex-", dir=_codex_runtime_root()) as temporary:
        home = Path(temporary).resolve()
        codex_home = home / ".codex"
        codex_home.mkdir(parents=True)
        source_codex_home = Path(str(config["codex_home"]))
        _link_auth(source_codex_home, codex_home)
        provider_lines, _provider_environment_key = _codex_provider_config(source_codex_home)
        route = ModelRoute(
            route_id="local.codex.primary",
            role="primary",
            provider="openai",
            model=str(config["model"]),
            snapshot=str(config["model"]),
            service_tier="default",
        )
        context = HarnessRenderContext(
            home=home,
            workspace=workspace,
            artifacts=artifacts,
            brief=brief,
            model_route=route,
            driver=NativeMcpDriver(
                Path(str(config["socket"])),
                executable=str(config["driver"]),
                server_name="cua",
            ),
        )
        harness = production_harness("codex", executable=str(config["codex"]))
        contract = harness.render(context, skill_files=_load_skill(config))
        for rendered in contract.config_files:
            rendered.path.parent.mkdir(parents=True, exist_ok=True)
            content = rendered.content
            if rendered.path == codex_home / "config.toml":
                lines = content.decode("utf-8").splitlines()
                lines[1:1] = [
                    "model_reasoning_effort = " + json.dumps(str(config["reasoning_effort"])),
                    *provider_lines,
                ]
                content = ("\n".join(lines) + "\n").encode("utf-8")
            rendered.path.write_bytes(content)
        provider_environment = _codex_provider_environment(source_codex_home)
        environment = clean_environment(
            {
                **dict(contract.environment),
                **gui_environment,
                **provider_environment,
                "PATH": os.environ.get("PATH", contract.environment["PATH"]),
            }
        )
        with (
            brief.open("rb") as stdin,
            events_path.open("wb") as stdout,
            stderr_path.open("wb") as stderr,
        ):
            process = subprocess.Popen(
                list(contract.argv),
                cwd=contract.cwd,
                env=environment,
                stdin=stdin,
                stdout=stdout,
                stderr=stderr,
                shell=False,
                **_new_process_group_options(detached=False),
            )
            _write_json(
                artifacts / "codex-process.json",
                {"pid": process.pid, "process_group": process.pid},
            )
            returncode, timed_out, terminated_after_terminal_event = _wait_for_codex(
                process,
                events_path,
                float(config["timeout_seconds"]) - 30.0,
            )
        telemetry, terminal_failure = _codex_telemetry(events_path, route)
        _write_json(artifacts / "codex-telemetry.json", telemetry)
        _write_json(
            run_path,
            {
                "duration_ms": int((time.monotonic() - started) * 1000),
                "exit_code": returncode,
                "timed_out": timed_out,
                "terminated_after_terminal_event": terminated_after_terminal_event,
                "terminal_failure": terminal_failure,
            },
        )
        return 0 if returncode == 0 and terminal_failure is None else 1


def launcher_main(config: Mapping[str, Any]) -> int:
    import argparse

    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    arguments = parser.parse_args()
    workspace = arguments.workspace.resolve()
    artifacts = arguments.artifacts.resolve()
    materialized_bundle = (artifacts.parent / "inputs" / "artifacts").resolve()
    app_bundle = Path(str(config["task_bundle"])).resolve()
    started = time.monotonic()
    try:
        descriptor = load_launch_descriptor(materialized_bundle, str(config["platform"]))
        semantics = descriptor.get("semantics", {})
        brief = semantics.get("brief")
        if not isinstance(brief, str) or not brief:
            raise ValueError("launch descriptor has no participant brief")
        variables = _descriptor_variables(app_bundle, workspace)
        variables["artifacts"] = str(artifacts)
        gui_environment = {
            str(key): str(value) for key, value in dict(config.get("gui_environment", {})).items()
        }
        _launch_apps(
            descriptor,
            variables,
            artifacts,
            gui_environment,
        )
        returncode = _run_codex(
            {**config, "brief": brief},
            workspace,
            artifacts,
            materialized_bundle,
            gui_environment,
        )
        _write_json(
            artifacts / "launcher-run.json",
            {
                "duration_ms": int((time.monotonic() - started) * 1000),
                "status": "completed" if returncode == 0 else "codex_failed",
            },
        )
        return returncode
    except Exception as error:  # noqa: BLE001 - launcher must persist diagnostics
        _write_json(
            artifacts / "launcher-run.json",
            {
                "duration_ms": int((time.monotonic() - started) * 1000),
                "status": "launcher_error",
                "error": f"{type(error).__name__}: {error}",
            },
        )
        print(f"launcher failed: {type(error).__name__}: {error}", file=sys.stderr)
        return 1


def _write_launcher(path: Path, config: Mapping[str, Any]) -> None:
    module_path = Path(__file__).resolve()
    serialized = json.dumps(config, sort_keys=True, separators=(",", ":"))
    source = f"""#!/usr/bin/env python3
import importlib.util
import json
import sys

MODULE_PATH = {str(module_path)!r}
SPEC = importlib.util.spec_from_file_location("cdb_compare_drivers", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load local comparison launcher")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)
CONFIG = json.loads({serialized!r})
raise SystemExit(MODULE.launcher_main(CONFIG))
"""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source, encoding="utf-8", newline="\n")
    if os.name != "nt":
        path.chmod(0o755)


def _windows_processes_for_path(path: Path) -> tuple[int, ...]:
    if os.name != "nt":
        return ()
    powershell = shutil.which("powershell.exe")
    if powershell is None:
        return ()
    needle = str(path.resolve()).casefold()
    powershell_needle = needle.replace("'", "''")
    script = f"""
$needle = '{powershell_needle}'
Get-CimInstance Win32_Process |
  Where-Object {{
    $_.ProcessId -ne $PID -and
    $_.CommandLine -and
    $_.CommandLine.ToLowerInvariant().Contains($needle)
  }} |
  ForEach-Object {{ $_.ProcessId }}
""".strip()
    encoded = base64.b64encode(script.encode("utf-16-le")).decode("ascii")
    try:
        completed = subprocess.run(
            [
                powershell,
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                encoded,
            ],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=10.0,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return ()
    if completed.returncode != 0:
        return ()
    return tuple(
        int(line)
        for line in completed.stdout.splitlines()
        if line.strip().isdigit() and int(line) > 0
    )


def _cleanup_trial_processes(trial_dir: Path) -> None:
    artifacts = trial_dir / "artifacts"
    paths = (artifacts / "codex-process.json", artifacts / "app-processes.json")
    seen: set[int] = set()
    for path in paths:
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            continue
        records = document if isinstance(document, list) else [document]
        for record in reversed(records):
            if not isinstance(record, dict) or record.get("skipped") is True:
                continue
            pid = record.get("process_group", record.get("pid"))
            if isinstance(pid, int) and pid > 0 and pid not in seen:
                seen.add(pid)
                _terminate_process_group(pid)
    for pid in _windows_processes_for_path(trial_dir):
        if pid not in seen:
            seen.add(pid)
            _terminate_process_group(pid)


def _move_daemon_logs(daemon: DriverDaemon, trial_dir: Path | None) -> None:
    if trial_dir is None or not trial_dir.is_dir():
        return
    for source, name in (
        (daemon.stdout_path, "driver-daemon.stdout"),
        (daemon.stderr_path, "driver-daemon.stderr"),
    ):
        if source.is_file():
            destination = trial_dir / "artifacts" / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            try:
                shutil.move(str(source), destination)
            except OSError:
                pass


def _unavailable_foreground_disturbance_metrics(
    error: str | None = None,
    measurement: str | None = None,
) -> dict[str, Any]:
    return {
        "foreground_disturbance_available": False,
        "foreground_disturbance_measurement": measurement,
        "foreground_disturbance_error": error,
        "foreground_keyboard_focus_available": False,
        "foreground_keyboard_focus_error": error,
        "foreground_keyboard_focus_drops": None,
        "foreground_drag_available": False,
        "foreground_drag_error": error,
        "foreground_drag_interruptions": None,
        "foreground_drag_observations": None,
        "foreground_cursor_trajectory_available": False,
        "foreground_cursor_trajectory_error": error,
        "foreground_cursor_trajectory_deviations": None,
        "foreground_disturbances": None,
        "focus_window_transitions": None,
        "cursor_deviation_episodes": None,
    }


def _non_negative_integer(value: Any) -> int | None:
    if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
        return value
    return None


def _optional_non_negative_integer(value: Any) -> int | None:
    if value is None:
        return None
    return _non_negative_integer(value)


def _foreground_evidence_error(summary_path: Path, summary: Mapping[str, Any]) -> str | None:
    evidence_name = summary.get("evidence_path")
    if not isinstance(evidence_name, str) or not evidence_name:
        return "foreground disturbance evidence path is missing"
    relative_path = Path(evidence_name)
    if relative_path.is_absolute() or len(relative_path.parts) != 1:
        return "foreground disturbance evidence path is invalid"
    evidence_path = summary_path.parent / relative_path
    try:
        with evidence_path.open(encoding="utf-8") as evidence_handle:
            first_line = next(
                (line for line in evidence_handle if line.strip()),
                None,
            )
        if first_line is None:
            return "foreground disturbance evidence is empty"
        first_record = json.loads(first_line)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        return f"foreground disturbance evidence unavailable: {type(error).__name__}: {error}"
    if not isinstance(first_record, dict):
        return "foreground disturbance evidence record must be a JSON object"
    if not isinstance(first_record.get("observed_at"), str):
        return "foreground disturbance evidence record has no timestamp"
    monotonic_ns = first_record.get("monotonic_ns")
    if not isinstance(monotonic_ns, int) or isinstance(monotonic_ns, bool):
        return "foreground disturbance evidence record has no monotonic timestamp"
    return None


def _foreground_disturbance_metrics(trial_dir: Path) -> dict[str, Any]:
    summary_path = trial_dir / "observer" / "foreground-disturbance.json"
    try:
        summary = json.loads(summary_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        return _unavailable_foreground_disturbance_metrics(f"{type(error).__name__}: {error}")
    if not isinstance(summary, dict):
        return _unavailable_foreground_disturbance_metrics(
            "foreground disturbance summary must be a JSON object"
        )
    measurement = summary.get("measurement")
    if not isinstance(measurement, str):
        measurement = None
    result = _unavailable_foreground_disturbance_metrics(measurement=measurement)
    result["focus_window_transitions"] = _optional_non_negative_integer(
        summary.get("focus_window_transitions")
    )
    result["cursor_deviation_episodes"] = _optional_non_negative_integer(
        summary.get("cursor_deviation_episodes")
    )
    evidence_error = _foreground_evidence_error(summary_path, summary)
    if evidence_error is not None:
        unavailable = _unavailable_foreground_disturbance_metrics(
            error=evidence_error,
            measurement=measurement,
        )
        unavailable["focus_window_transitions"] = result["focus_window_transitions"]
        unavailable["cursor_deviation_episodes"] = result["cursor_deviation_episodes"]
        return unavailable

    component_specs = (
        (
            "foreground_keyboard_focus",
            "foreground_keyboard_focus_drops",
            None,
        ),
        (
            "foreground_drag",
            "foreground_drag_interruptions",
            "foreground_drag_observations",
        ),
        (
            "foreground_cursor_trajectory",
            "foreground_cursor_trajectory_deviations",
            None,
        ),
    )
    component_values: dict[str, int] = {}
    validation_errors: list[str] = []
    for prefix, value_key, auxiliary_key in component_specs:
        available_key = f"{prefix}_available"
        error_key = f"{prefix}_error"
        claimed_available = summary.get(available_key) is True
        component_error = summary.get(error_key)
        if component_error is not None and not isinstance(component_error, str):
            component_error = f"invalid {error_key}"
        value = _non_negative_integer(summary.get(value_key))
        auxiliary_value = (
            _non_negative_integer(summary.get(auxiliary_key)) if auxiliary_key is not None else None
        )
        if claimed_available and value is None:
            claimed_available = False
            component_error = f"invalid {value_key}"
        if claimed_available and auxiliary_key is not None and auxiliary_value is None:
            claimed_available = False
            component_error = f"invalid {auxiliary_key}"
        if not claimed_available:
            value = None
            auxiliary_value = None
            component_error = component_error or f"{prefix} evidence unavailable"
            validation_errors.append(component_error)
        else:
            assert value is not None
            component_values[prefix] = value
        result[available_key] = claimed_available
        result[error_key] = None if claimed_available else component_error
        result[value_key] = value
        if auxiliary_key is not None:
            result[auxiliary_key] = auxiliary_value

    aggregate = _non_negative_integer(summary.get("foreground_disturbances"))
    expected_aggregate = (
        sum(component_values.values()) if len(component_values) == len(component_specs) else None
    )
    if summary.get("available") is not True:
        validation_errors.append(
            str(summary.get("error") or "foreground disturbance suite unavailable")
        )
    elif expected_aggregate is None:
        validation_errors.append("one or more foreground disturbance components unavailable")
    elif aggregate != expected_aggregate:
        validation_errors.append("foreground_disturbances does not equal the component sum")
    else:
        result["foreground_disturbance_available"] = True
        result["foreground_disturbance_error"] = None
        result["foreground_disturbances"] = aggregate
        return result

    result["foreground_disturbance_error"] = "; ".join(dict.fromkeys(validation_errors))
    return result


def _is_successful_input_action(item: Mapping[str, Any]) -> bool:
    if item.get("status") != "completed":
        return False
    tool = item.get("tool")
    if tool in INPUT_ACTIONS:
        return True
    arguments = item.get("arguments")
    if not isinstance(arguments, dict):
        return False
    if tool == "page":
        return arguments.get("action") in LEGACY_PAGE_INPUT_ACTIONS
    return tool == "browser_dialog" and arguments.get("action") in {
        "accept",
        "dismiss",
    }


def _codex_cua_metrics(trial_dir: Path) -> tuple[int, int] | None:
    events_path = trial_dir / "artifacts" / "codex-events.jsonl"
    cua_calls = 0
    input_actions = 0
    parsed_event = False
    try:
        with events_path.open(encoding="utf-8") as events:
            for line in events:
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(event, dict):
                    continue
                parsed_event = True
                item = event.get("item")
                if (
                    event.get("type") != "item.completed"
                    or not isinstance(item, dict)
                    or item.get("type") != "mcp_tool_call"
                    or item.get("server") != "cua"
                ):
                    continue
                cua_calls += 1
                if _is_successful_input_action(item):
                    input_actions += 1
    except (OSError, UnicodeDecodeError):
        return None
    return (cua_calls, input_actions) if parsed_event else None


def _recorded_cua_metrics(trial_dir: Path) -> tuple[int, int]:
    cua_calls = 0
    input_actions = 0
    recording = trial_dir / "observer" / "cua-driver-recording"
    for action_path in sorted(recording.glob("turn-*/action.json")):
        try:
            action = json.loads(action_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            continue
        if not isinstance(action, dict):
            continue
        cua_calls += 1
        if action.get("tool") in INPUT_ACTIONS and action.get("result_error") is False:
            input_actions += 1
    return cua_calls, input_actions


def _trial_cua_metrics(trial_dir: Path) -> tuple[int, int]:
    metrics = _codex_cua_metrics(trial_dir)
    return metrics if metrics is not None else _recorded_cua_metrics(trial_dir)


def extract_trial_metrics(
    trial_dir: Path,
    task: str,
    version: str,
    trial_id: str,
) -> TrialMetrics:
    disturbance_metrics = _foreground_disturbance_metrics(trial_dir)
    result_path = trial_dir / "result.json"
    try:
        result = json.loads(result_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        return TrialMetrics(
            task=task,
            version=version,
            trial_id=trial_id,
            trial_dir=str(trial_dir),
            passed=False,
            score=None,
            total_ms=None,
            cua_calls=0,
            input_actions=0,
            termination="missing_result",
            error=f"{type(error).__name__}: {error}",
            **disturbance_metrics,
        )
    cua_calls, input_actions = _trial_cua_metrics(trial_dir)
    evaluation = result.get("evaluation")
    passed = bool(isinstance(evaluation, dict) and evaluation.get("passed") is True)
    score_value = evaluation.get("score") if isinstance(evaluation, dict) else None
    score = (
        float(score_value)
        if isinstance(score_value, (int, float)) and not isinstance(score_value, bool)
        else None
    )
    termination = str(result.get("status", "unknown"))
    codex_run_path = trial_dir / "artifacts" / "codex-run.json"
    if termination == "completed" and codex_run_path.is_file():
        try:
            codex_run = json.loads(codex_run_path.read_text(encoding="utf-8"))
            if codex_run.get("timed_out") is True:
                termination = "codex_timeout"
            elif codex_run.get("terminal_failure"):
                termination = str(codex_run["terminal_failure"])
            elif codex_run.get("exit_code") != 0:
                termination = f"codex_exit_{codex_run.get('exit_code')}"
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            pass
    launcher_path = trial_dir / "artifacts" / "launcher-run.json"
    if termination == "completed" and launcher_path.is_file():
        try:
            launcher = json.loads(launcher_path.read_text(encoding="utf-8"))
            if launcher.get("status") not in {None, "completed"}:
                termination = str(launcher["status"])
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            pass
    if termination == "completed" and isinstance(evaluation, dict):
        detail = evaluation.get("detail")
        agent_exit_code = detail.get("agent_exit_code") if isinstance(detail, dict) else None
        if (
            isinstance(agent_exit_code, int)
            and not isinstance(agent_exit_code, bool)
            and agent_exit_code != 0
        ):
            termination = f"agent_exit_{agent_exit_code}"
    tokens: Mapping[str, int] | None = None
    telemetry_path = trial_dir / "artifacts" / "codex-telemetry.json"
    try:
        telemetry = json.loads(telemetry_path.read_text(encoding="utf-8"))
        raw_usage = telemetry.get("usage")
        if isinstance(raw_usage, dict):
            usage = {
                str(key): int(value)
                for key, value in raw_usage.items()
                if isinstance(value, int) and not isinstance(value, bool)
            }
            tokens = usage or None
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        pass
    elapsed = result.get("elapsed_ms")
    return TrialMetrics(
        task=task,
        version=version,
        trial_id=trial_id,
        trial_dir=str(trial_dir),
        passed=passed,
        score=score,
        total_ms=elapsed if isinstance(elapsed, int) else None,
        cua_calls=cua_calls,
        input_actions=input_actions,
        termination=termination,
        codex_tokens=tokens,
        error=result.get("error") if isinstance(result.get("error"), str) else None,
        **disturbance_metrics,
    )


def failed_trial_metrics(
    task: str,
    version: str,
    trial_id: str,
    error: Exception,
    trial_dir: Path | None = None,
) -> TrialMetrics:
    disturbance_metrics = (
        _foreground_disturbance_metrics(trial_dir)
        if trial_dir is not None
        else _unavailable_foreground_disturbance_metrics()
    )
    return TrialMetrics(
        task=task,
        version=version,
        trial_id=trial_id,
        trial_dir=str(trial_dir) if trial_dir is not None else None,
        passed=False,
        score=None,
        total_ms=None,
        cua_calls=0,
        input_actions=0,
        termination="orchestration_error",
        error=f"{type(error).__name__}: {error}",
        **disturbance_metrics,
    )


def _codex_token_value(usage: Mapping[str, int] | None, key: str) -> int | None:
    if not isinstance(usage, Mapping):
        return None
    value = usage.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        return None
    return value


def compare_pair(baseline: TrialMetrics, candidate: TrialMetrics) -> dict[str, Any]:
    def delta(candidate_value: int | float | None, baseline_value: int | float | None):
        if candidate_value is None or baseline_value is None:
            return None
        return candidate_value - baseline_value

    score_delta = delta(candidate.score, baseline.score)
    if baseline.termination != "completed" or candidate.termination != "completed":
        signal = "incomplete"
    elif baseline.score is None or candidate.score is None:
        signal = "incomplete"
    elif not baseline.passed and candidate.passed:
        signal = "improved"
    elif baseline.passed and not candidate.passed:
        signal = "regressed"
    elif score_delta is not None and score_delta > 0:
        signal = "improved"
    elif score_delta is not None and score_delta < 0:
        signal = "regressed"
    else:
        signal = "unchanged"
    return {
        "task": baseline.task,
        "baseline": asdict(baseline),
        "candidate": asdict(candidate),
        "delta": {
            "score": score_delta,
            "total_ms": delta(candidate.total_ms, baseline.total_ms),
            "cua_calls": delta(candidate.cua_calls, baseline.cua_calls),
            "input_actions": delta(candidate.input_actions, baseline.input_actions),
            "foreground_keyboard_focus_drops": delta(
                candidate.foreground_keyboard_focus_drops,
                baseline.foreground_keyboard_focus_drops,
            ),
            "foreground_drag_interruptions": delta(
                candidate.foreground_drag_interruptions,
                baseline.foreground_drag_interruptions,
            ),
            "foreground_cursor_trajectory_deviations": delta(
                candidate.foreground_cursor_trajectory_deviations,
                baseline.foreground_cursor_trajectory_deviations,
            ),
            "focus_window_transitions": delta(
                candidate.focus_window_transitions,
                baseline.focus_window_transitions,
            ),
            "cursor_deviation_episodes": delta(
                candidate.cursor_deviation_episodes,
                baseline.cursor_deviation_episodes,
            ),
            "foreground_disturbances": delta(
                candidate.foreground_disturbances,
                baseline.foreground_disturbances,
            ),
            "input_tokens": delta(
                _codex_token_value(candidate.codex_tokens, "input_tokens"),
                _codex_token_value(baseline.codex_tokens, "input_tokens"),
            ),
            "cached_tokens": delta(
                _codex_token_value(candidate.codex_tokens, "cache_read_tokens"),
                _codex_token_value(baseline.codex_tokens, "cache_read_tokens"),
            ),
            "output_tokens": delta(
                _codex_token_value(candidate.codex_tokens, "output_tokens"),
                _codex_token_value(baseline.codex_tokens, "output_tokens"),
            ),
        },
        "signal": signal,
    }


def build_comparisons(
    trials: Sequence[TrialMetrics], baseline: str, candidate: str | None
) -> list[dict[str, Any]]:
    if candidate is None:
        return []
    indexed = {(trial.task, trial.version): trial for trial in trials}
    comparisons: list[dict[str, Any]] = []
    for task in SHARED_TASKS:
        baseline_trial = indexed.get((task, baseline))
        candidate_trial = indexed.get((task, candidate))
        if baseline_trial is not None and candidate_trial is not None:
            comparisons.append(compare_pair(baseline_trial, candidate_trial))
    return comparisons


def _markdown_value(value: Any) -> str:
    if value is None:
        return "—"
    if isinstance(value, bool):
        return "yes" if value else "no"
    if isinstance(value, float):
        return f"{value:.4g}"
    return str(value).replace("|", "\\|")


def render_markdown(report: Mapping[str, Any]) -> str:
    lines = [
        "# Cua Driver Local Diagnostic Comparison",
        "",
        "> Non-certifying local benchmark output.",
        "",
        (
            "> Foreground disturbances are the sum of unexpected keyboard-focus "
            "drops, interrupted foreground drags, and physical cursor deviations "
            "from the initial static trajectory. These diagnostics do not affect "
            "scores, pass/fail, or comparison signals."
        ),
        "",
        "## Trials",
        "",
        (
            "| Task | Version | Pass | Score | Total ms | Cua calls | "
            "Input actions | Focus drops | Drag interruptions | "
            "Cursor deviations | FG disturbances | Input tokens | "
            "Cached tokens | Output tokens | Termination |"
        ),
        (
            "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | "
            "---: | ---: | ---: | ---: | ---: | --- |"
        ),
    ]
    for trial in report["trials"]:
        codex_tokens = trial.get("codex_tokens")
        lines.append(
            "| "
            + " | ".join(
                _markdown_value(value)
                for value in (
                    trial["task"],
                    trial["version"],
                    trial["passed"],
                    trial["score"],
                    trial["total_ms"],
                    trial["cua_calls"],
                    trial["input_actions"],
                    trial.get("foreground_keyboard_focus_drops"),
                    trial.get("foreground_drag_interruptions"),
                    trial.get("foreground_cursor_trajectory_deviations"),
                    trial.get("foreground_disturbances"),
                    _codex_token_value(codex_tokens, "input_tokens"),
                    _codex_token_value(codex_tokens, "cache_read_tokens"),
                    _codex_token_value(codex_tokens, "output_tokens"),
                    trial["termination"],
                )
            )
            + " |"
        )
    lines.extend(
        [
            "",
            "## Foreground Disturbance Availability",
            "",
            ("| Task | Version | Suite | Keyboard focus | Drag | Cursor trajectory | Error |"),
            "| --- | --- | --- | --- | --- | --- | --- |",
        ]
    )
    for trial in report["trials"]:
        component_errors = [
            trial.get("foreground_disturbance_error"),
            trial.get("foreground_keyboard_focus_error"),
            trial.get("foreground_drag_error"),
            trial.get("foreground_cursor_trajectory_error"),
        ]
        error = "; ".join(dict.fromkeys(str(value) for value in component_errors if value))
        lines.append(
            "| "
            + " | ".join(
                _markdown_value(value)
                for value in (
                    trial["task"],
                    trial["version"],
                    trial.get("foreground_disturbance_available", False),
                    trial.get("foreground_keyboard_focus_available", False),
                    trial.get("foreground_drag_available", False),
                    trial.get("foreground_cursor_trajectory_available", False),
                    error or None,
                )
            )
            + " |"
        )
    lines.extend(
        [
            "",
            "## Baseline vs Candidate",
            "",
            (
                "| Task | Baseline score | Candidate score | Score Δ | "
                "Time Δ ms | Call Δ | Action Δ | Focus-drop Δ | Drag Δ | "
                "Cursor Δ | Disturbance Δ | Input token Δ | Cached token Δ | "
                "Output token Δ | Signal |"
            ),
            (
                "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | "
                "---: | ---: | ---: | ---: | ---: | ---: | --- |"
            ),
        ]
    )
    for comparison in report["comparisons"]:
        lines.append(
            "| "
            + " | ".join(
                _markdown_value(value)
                for value in (
                    comparison["task"],
                    comparison["baseline"]["score"],
                    comparison["candidate"]["score"],
                    comparison["delta"]["score"],
                    comparison["delta"]["total_ms"],
                    comparison["delta"]["cua_calls"],
                    comparison["delta"]["input_actions"],
                    comparison["delta"]["foreground_keyboard_focus_drops"],
                    comparison["delta"]["foreground_drag_interruptions"],
                    comparison["delta"]["foreground_cursor_trajectory_deviations"],
                    comparison["delta"]["foreground_disturbances"],
                    comparison["delta"]["input_tokens"],
                    comparison["delta"]["cached_tokens"],
                    comparison["delta"]["output_tokens"],
                    comparison["signal"],
                )
            )
            + " |"
        )
    return "\n".join(lines)


def build_plan(config: ComparisonConfig) -> dict[str, Any]:
    releases = discover_driver_releases(config.drivers_root, config.platform)
    baseline = require_release(releases, config.baseline)
    candidate_name = config.candidate if config.candidate != config.baseline else None
    candidate = require_release(releases, candidate_name) if candidate_name is not None else None
    selected_releases = (baseline,) if candidate is None else (baseline, candidate)
    manifests = [task_path(config.tasks_root, task) for task in config.tasks]
    for manifest in manifests:
        load_launch_descriptor(manifest.parent, config.platform)
    return {
        "platform": config.platform,
        "diagnostic": True,
        "baseline": asdict(baseline),
        "candidate": asdict(candidate) if candidate is not None else None,
        "tasks": list(config.tasks),
        "trials": [
            {"task": task, "version": release.version}
            for release in selected_releases
            for task in config.tasks
        ],
    }


def _trial_id(task: str, version: str, run_stamp: str) -> str:
    return f"{task}-{version}-{run_stamp}"


def _run_trial(
    config: ComparisonConfig,
    release: DriverRelease,
    task: str,
    run_stamp: str,
) -> TrialMetrics:
    trial_id = _trial_id(task, release.version, run_stamp)
    trial_root = config.output / "trials" / trial_id
    launcher = config.output / "launchers" / f"{trial_id}.py"
    runtime_log = config.output / "runtime" / f"{trial_id}.driver-daemon"
    gui_environment = _gui_environment()
    daemon: DriverDaemon | None = None
    materialized_trial: Path | None = None
    try:
        daemon = DriverDaemon(release, runtime_log, gui_environment)
        with daemon:
            _write_launcher(
                launcher,
                {
                    "codex": str(config.codex),
                    "codex_home": str(config.codex_home),
                    "driver": str(release.binary),
                    "socket": daemon.endpoint,
                    "model": config.model,
                    "reasoning_effort": config.reasoning_effort,
                    "timeout_seconds": config.timeout_seconds,
                    "platform": config.platform,
                    "task_bundle": str(task_path(config.tasks_root, task).parent),
                    "gui_environment": gui_environment,
                    "skill_kind": release.skill_kind,
                    "skill_source": (str(release.skill_source) if release.skill_source else None),
                },
            )
            agent_environment = _codex_provider_environment(config.codex_home)
            with _local_adapter_override(
                release,
                daemon.endpoint,
                gui_environment,
                agent_environment,
            ):
                evaluator_options = _evaluator_node_options(
                    task_path(config.tasks_root, task).parent, config.platform
                )
                _exit_code, materialized_trial, _result = engine.run_trial(
                    task_path=task_path(config.tasks_root, task),
                    agent_command=launcher,
                    out=config.output / "trials",
                    trial_id=trial_id,
                    timeout_seconds=config.timeout_seconds,
                    environment_name="task-local-cua-smoke",
                    **evaluator_options,
                )
    except Exception as error:  # noqa: BLE001 - continue the comparison matrix
        if materialized_trial is None and trial_root.is_dir():
            materialized_trial = trial_root
        return failed_trial_metrics(task, release.version, trial_id, error, materialized_trial)
    finally:
        if materialized_trial is not None:
            _cleanup_trial_processes(materialized_trial)
        if daemon is not None:
            _move_daemon_logs(daemon, materialized_trial)
    assert materialized_trial is not None
    return extract_trial_metrics(materialized_trial, task, release.version, trial_id)


def run_comparison(config: ComparisonConfig) -> tuple[dict[str, Any], Path, Path]:
    if config.timeout_seconds <= 30:
        raise ValueError("timeout must be greater than 30 seconds")
    releases = discover_driver_releases(config.drivers_root, config.platform)
    baseline = require_release(releases, config.baseline)
    candidate_name = config.candidate if config.candidate != config.baseline else None
    candidate = require_release(releases, candidate_name) if candidate_name is not None else None
    selected_releases = (baseline,) if candidate is None else (baseline, candidate)
    manifests = [task_path(config.tasks_root, task) for task in config.tasks]
    codex_version = preflight(config, selected_releases, manifests)
    config.output.mkdir(parents=True, exist_ok=False)
    run_stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    trials: list[TrialMetrics] = []
    for release in selected_releases:
        for task in config.tasks:
            trials.append(_run_trial(config, release, task, run_stamp))
    comparisons = build_comparisons(trials, config.baseline, candidate_name)
    report = {
        "schema_version": "1",
        "generated_at": datetime.now(UTC).isoformat(),
        "diagnostic": True,
        "certifying": False,
        "platform": config.platform,
        "baseline": config.baseline,
        "candidate": candidate_name,
        "drivers": {
            "baseline": {
                "version": baseline.version,
                "binary": str(baseline.binary),
                "skill_source": (str(baseline.skill_source) if baseline.skill_source else None),
            },
            "candidate": (
                {
                    "version": candidate.version,
                    "binary": str(candidate.binary),
                    "skill_source": (
                        str(candidate.skill_source) if candidate.skill_source else None
                    ),
                }
                if candidate is not None
                else None
            ),
        },
        "harness": {
            "name": "codex-cli",
            "version": codex_version,
            "model": config.model,
            "reasoning_effort": config.reasoning_effort,
        },
        "foreground_disturbance": {
            "measurement": FOREGROUND_DISTURBANCE_MEASUREMENT,
            "platform": "linux-x11",
            "sample_hz": FOREGROUND_SAMPLE_HZ,
            "components": {
                "foreground_keyboard_focus_drops": {
                    "semantics": "valid foreground focus becomes unavailable",
                    "valid_window_transfers_counted": False,
                },
                "foreground_drag_interruptions": {
                    "semantics": (
                        "observed foreground drag ends without an intentional button release"
                    ),
                    "event_source": "XRecord",
                },
                "foreground_cursor_trajectory_deviations": {
                    "semantics": "physical cursor leaves the initial static reference",
                    "threshold_px": CURSOR_DEVIATION_THRESHOLD_PX,
                    "rearm": "return_within_threshold",
                },
            },
            "raw_evidence": "observer/foreground-disturbance.ndjson",
            "legacy_secondary_fields": [
                "focus_window_transitions",
                "cursor_deviation_episodes",
            ],
            "affects_signal": False,
        },
        "trials": [asdict(trial) for trial in trials],
        "comparisons": comparisons,
    }
    json_path = config.output / "comparison.json"
    markdown_path = config.output / "comparison.md"
    _write_json(json_path, report)
    markdown_path.write_text(render_markdown(report), encoding="utf-8", newline="\n")
    return report, json_path, markdown_path
