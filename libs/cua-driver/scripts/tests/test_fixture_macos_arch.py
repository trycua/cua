from __future__ import annotations

import os
from pathlib import Path
import shutil
import shlex
import subprocess


REPO_ROOT = Path(__file__).resolve().parents[4]
BUILD_SCRIPT = REPO_ROOT / "libs/cua-driver/tests/fixtures/build/macos.sh"


def _bash_path(path: Path) -> str:
    if os.name != "nt":
        return str(path)
    drive = path.drive.rstrip(":").lower()
    return f"/mnt/{drive}/{path.relative_to(path.anchor).as_posix()}"


def _write_executable(path: Path, body: str) -> None:
    path.write_text(body, encoding="utf-8", newline="\n")
    path.chmod(0o755)


def _fixture_tree(tmp_path: Path) -> tuple[Path, dict[str, str], Path]:
    driver_root = tmp_path / "cua-driver"
    script = driver_root / "tests/fixtures/build/macos.sh"
    script.parent.mkdir(parents=True)
    shutil.copy2(BUILD_SCRIPT, script)
    script.chmod(0o755)

    source_dir = driver_root / "tests/fixtures/apps/macos/appkit"
    source_dir.mkdir(parents=True)
    (source_dir / "App.swift").write_text('print("fixture")\n', encoding="utf-8")
    (driver_root / "rust/test-apps").mkdir(parents=True)

    fake_bin = tmp_path / "fake-bin"
    fake_bin.mkdir()
    tool_log = tmp_path / "tools.log"
    _write_executable(
        fake_bin / "uname",
        '#!/usr/bin/env bash\nprintf "%s\\n" "$FAKE_UNAME_ARCH"\n',
    )
    _write_executable(
        fake_bin / "xcrun",
        """#!/usr/bin/env bash
printf 'xcrun %s\\n' "$*" >> "$FAKE_TOOL_LOG"
if [[ "$1" == "swiftc" ]]; then
    output=""
    while [[ $# -gt 0 ]]; do
        if [[ "$1" == "-o" ]]; then
            output="$2"
            break
        fi
        shift
    done
    [[ -n "$output" ]] || exit 91
    : > "$output"
    exit 0
fi
if [[ "$1" == "lipo" && "$2" == "-archs" ]]; then
    printf '%s\\n' "$FAKE_LIPO_ARCHS"
    exit 0
fi
exit 92
""",
    )
    _write_executable(
        fake_bin / "lipo",
        """#!/usr/bin/env bash
printf 'lipo %s\\n' "$*" >> "$FAKE_TOOL_LOG"
if [[ "$1" == "-archs" ]]; then
    printf '%s\\n' "$FAKE_LIPO_ARCHS"
    exit 0
fi
exit 93
""",
    )

    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{_bash_path(fake_bin)}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            "FAKE_TOOL_LOG": _bash_path(tool_log),
            "FAKE_UNAME_ARCH": "x86_64",
            "FAKE_LIPO_ARCHS": "x86_64",
        }
    )
    return script, env, tool_log


def _run(script: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    if os.name == "nt":
        keys = (
            "PATH",
            "FAKE_TOOL_LOG",
            "FAKE_UNAME_ARCH",
            "FAKE_LIPO_ARCHS",
            "CUA_FIXTURE_ARCH",
        )
        assignments = " ".join(f"{key}={shlex.quote(env[key])}" for key in keys if key in env)
        command = f"{assignments} bash {shlex.quote(_bash_path(script))} --only appkit"
        return subprocess.run(
            ["bash", "-lc", command],
            check=False,
            capture_output=True,
            text=True,
        )
    return subprocess.run(
        ["bash", _bash_path(script), "--only", "appkit"],
        check=False,
        capture_output=True,
        text=True,
        env=env,
    )


def test_fixture_build_targets_and_verifies_intel_host(tmp_path: Path) -> None:
    script, env, tool_log = _fixture_tree(tmp_path)

    result = _run(script, env)

    assert result.returncode == 0, result.stderr
    calls = tool_log.read_text(encoding="utf-8")
    assert "-target x86_64-apple-macos13.0" in calls
    assert "lipo -archs" in calls


def test_fixture_build_rejects_declared_arch_that_differs_from_host(tmp_path: Path) -> None:
    script, env, tool_log = _fixture_tree(tmp_path)
    env["CUA_FIXTURE_ARCH"] = "arm64"

    result = _run(script, env)

    assert result.returncode != 0
    assert "architecture" in result.stderr.lower()
    assert not tool_log.exists(), "architecture mismatch must fail before compilation"


def test_fixture_build_rejects_binary_missing_host_architecture(tmp_path: Path) -> None:
    script, env, _ = _fixture_tree(tmp_path)
    env["FAKE_LIPO_ARCHS"] = "arm64"

    result = _run(script, env)

    assert result.returncode != 0
    assert "architecture" in result.stderr.lower()
