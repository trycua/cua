"""Focused tests for the macOS Lume release-certification runner.

These exercise the runner's shell helpers directly by sourcing each script in
library-only mode (``CUA_E2E_RUNNER_LIB_ONLY=1``), so status matching, build
namespace selection, option validation, retry bookkeeping, and daemon
restoration are covered without a macOS GUI session.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import sqlite3
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[4]
RUN_ALL = REPO_ROOT / "libs/cua-driver/tests/runners/macos-lume/run-all.sh"
SEED_TCC = REPO_ROOT / "libs/cua-driver/tests/runners/macos-lume/seed-tcc.sh"
SEED_TCC_GUEST = REPO_ROOT / "libs/cua-driver/tests/runners/macos-lume/seed-tcc-guest.sh"
RUN_RUST_E2E = REPO_ROOT / "scripts/ci/macos/run-rust-e2e.sh"
ELECTRON_BUILD = REPO_ROOT / "libs/cua-driver/tests/fixtures/apps/cross-platform/electron/build.sh"
ELECTRON_LOCK = (
    REPO_ROOT / "libs/cua-driver/tests/fixtures/apps/cross-platform/electron/package-lock.json"
)
TAURI_BUILD = REPO_ROOT / "libs/cua-driver/tests/fixtures/apps/cross-platform/tauri/build.sh"

requires_jq = pytest.mark.skipif(shutil.which("jq") is None, reason="jq is required")


def _write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def _run(
    script: Path,
    snippet: str,
    env: dict[str, str] | None = None,
    args: list[str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Source ``script`` in library-only mode and run ``snippet`` against it."""
    merged = os.environ.copy()
    merged["CUA_E2E_RUNNER_LIB_ONLY"] = "1"
    merged.update(env or {})
    body = f'source "{script}"\n{snippet}\n'
    return subprocess.run(
        ["bash", "-c", body, "runner-test", *(args or [])],
        capture_output=True,
        text=True,
        env=merged,
        check=False,
    )


# The runner derives the installed daemon path from HOME on purpose, so tests
# rebind it after sourcing instead of adding an environment seam to the runner.
INSTALLED_BIN_PRELUDE = 'INSTALLED_BIN="$TEST_INSTALLED_BIN"\n'


def _fields(output: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    for line in output.splitlines():
        if "=" in line:
            key, _, value = line.partition("=")
            fields[key] = value
    return fields


# --------------------------------------------------------------------------
# Syntax
# --------------------------------------------------------------------------


@pytest.mark.parametrize(
    "script",
    [RUN_ALL, RUN_RUST_E2E, SEED_TCC, SEED_TCC_GUEST, ELECTRON_BUILD, TAURI_BUILD],
    ids=lambda path: f"{path.parent.name}/{path.name}",
)
def test_runner_scripts_have_valid_bash_syntax(script: Path) -> None:
    assert subprocess.run(["bash", "-n", str(script)], check=False).returncode == 0


@pytest.mark.parametrize("script", [RUN_ALL, RUN_RUST_E2E], ids=lambda path: path.name)
def test_library_only_mode_fails_closed_when_script_is_executed(script: Path) -> None:
    completed = subprocess.run(
        ["bash", str(script)],
        capture_output=True,
        text=True,
        env={**os.environ, "CUA_E2E_RUNNER_LIB_ONLY": "1"},
        check=False,
    )
    assert completed.returncode == 2
    assert "valid only when this script is sourced" in completed.stderr


# --------------------------------------------------------------------------
# Status matching cannot break the producer's stdout
# --------------------------------------------------------------------------


def _noisy_status_bin(tmp_path: Path) -> Path:
    """A CLI that reports the mode and then keeps writing to stdout.

    The real driver CLI panics when its stdout closes mid-write, so a status
    check that short-circuits the producer turns a match into a failure under
    ``pipefail``.
    """
    fake = tmp_path / "fake-bin/cua-driver-local"
    _write_executable(
        fake,
        """printf 'permission mode: unrestricted\n'
i=0
while [ "$i" -lt 20000 ]; do
    printf 'status noise line %s\n' "$i" || exit 101
    i=$((i + 1))
done
exit 0
""",
    )
    return fake


def test_short_circuiting_the_producer_would_fail_under_pipefail(tmp_path: Path) -> None:
    fake = _noisy_status_bin(tmp_path)
    completed = subprocess.run(
        [
            "bash",
            "-c",
            'set -o pipefail; "$1" status | grep -Fq "permission mode: unrestricted"',
            "pipe-test",
            str(fake),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode != 0, "expected the closed-pipe producer to fail"


def test_status_match_captures_output_before_matching(tmp_path: Path) -> None:
    fake = _noisy_status_bin(tmp_path)
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE
        + 'if daemon_reports_permission_mode unrestricted; then echo "matched=yes"; '
        'else echo "matched=no"; fi',
        env={
            "TEST_INSTALLED_BIN": str(fake),
            "CUA_E2E_MACOS_DAEMON_SOCKET": str(tmp_path / "driver.sock"),
        },
    )
    assert completed.returncode == 0, completed.stderr
    assert "matched=yes" in completed.stdout


def test_status_match_rejects_the_wrong_mode_and_a_failing_cli(tmp_path: Path) -> None:
    standard = tmp_path / "fake-bin/standard"
    _write_executable(standard, "printf 'permission mode: standard\n'\n")
    broken = tmp_path / "fake-bin/broken"
    _write_executable(broken, "printf 'permission mode: unrestricted\n'\nexit 3\n")

    for binary, expected in ((standard, "matched=no"), (broken, "matched=no")):
        completed = _run(
            RUN_ALL,
            INSTALLED_BIN_PRELUDE
            + 'if daemon_reports_permission_mode unrestricted; then echo "matched=yes"; '
            'else echo "matched=no"; fi',
            env={"TEST_INSTALLED_BIN": str(binary)},
        )
        assert completed.returncode == 0, completed.stderr
        assert expected in completed.stdout


@pytest.mark.parametrize("script", [RUN_ALL, RUN_RUST_E2E], ids=lambda path: path.name)
def test_no_permission_mode_check_pipes_into_grep(script: Path) -> None:
    text = script.read_text(encoding="utf-8")
    assert re.search(r"\|\s*grep\s+[^\n]*-q", text) is None


# --------------------------------------------------------------------------
# SIP-off TCC seed helper
# --------------------------------------------------------------------------


def test_tcc_guest_seed_is_vm_and_sip_gated() -> None:
    text = SEED_TCC_GUEST.read_text(encoding="utf-8")
    assert re.search(r'\[\[ "\$\{MODEL\}" == VirtualMac\* \]\] \|\| fail', text)
    assert "csrutil status" in text
    assert 'fail "system TCC.db is SIP-protected' in text


def test_tcc_guest_seed_refuses_non_virtualmac_before_tcc_access() -> None:
    completed = subprocess.run(
        ["bash", str(SEED_TCC_GUEST)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 2
    assert "seed-tcc:" in completed.stderr
    assert "sqlite3" not in completed.stderr


def _guest_seed_assignment(name: str) -> str:
    text = SEED_TCC_GUEST.read_text(encoding="utf-8")
    match = re.search(rf'^{name}="(?P<body>.*?)"', text, re.DOTALL | re.MULTILINE)
    assert match is not None
    return match.group("body")


def test_tcc_guest_seed_grants_both_driver_permissions() -> None:
    text = SEED_TCC_GUEST.read_text(encoding="utf-8")
    sql_body = _guest_seed_assignment("SQL")
    assert re.search(
        r"\('kTCCServiceAccessibility','\$\{CLIENT_SQL\}',\$\{CLIENT_TYPE\},2,2,1,",
        sql_body,
    )
    assert re.search(
        r"\('kTCCServiceScreenCapture','\$\{CLIENT_SQL\}',\$\{CLIENT_TYPE\},2,2,1,",
        sql_body,
    )
    assert "auth_value" in sql_body
    assert "csreq" in sql_body
    assert "allowed" not in sql_body
    assert "auth_value=2" in text
    assert "com.trycua.driver.local" in text


def test_tcc_guest_seed_sql_executes_against_modern_tcc_schema(tmp_path: Path) -> None:
    sql_body = _guest_seed_assignment("SQL")
    verify_sql = _guest_seed_assignment("VERIFY_SQL")
    client = "com.trycua.driver.local"
    client_type = "0"
    csreq_hex = "01020304"
    substitutions = {
        "${CLIENT_SQL}": client,
        "${CLIENT_TYPE}": client_type,
        "${CSREQ_HEX}": csreq_hex,
    }
    for old, new in substitutions.items():
        sql_body = sql_body.replace(old, new)
        verify_sql = verify_sql.replace(old, new)

    db = tmp_path / "TCC.db"
    with sqlite3.connect(db) as conn:
        conn.executescript(
            """
            CREATE TABLE access(
              service TEXT NOT NULL,
              client TEXT NOT NULL,
              client_type INTEGER NOT NULL,
              auth_value INTEGER,
              auth_reason INTEGER,
              auth_version INTEGER,
              csreq BLOB,
              flags INTEGER,
              indirect_object_identifier_type INTEGER,
              indirect_object_identifier TEXT DEFAULT 'UNUSED',
              indirect_object_code_identity BLOB,
              last_modified INTEGER DEFAULT 0
            );
            """
        )
        conn.executescript(sql_body)
        row_count = conn.execute(verify_sql).fetchone()[0]
        rows = conn.execute(
            """
            SELECT service, client, client_type, auth_value, auth_reason,
                   auth_version, hex(csreq), indirect_object_identifier
              FROM access
             ORDER BY service
            """
        ).fetchall()

    assert row_count == 2
    assert rows == [
        (
            "kTCCServiceAccessibility",
            client,
            0,
            2,
            2,
            1,
            csreq_hex.upper(),
            "UNUSED",
        ),
        (
            "kTCCServiceScreenCapture",
            client,
            0,
            2,
            2,
            1,
            csreq_hex.upper(),
            "UNUSED",
        ),
    ]


def test_tcc_guest_seed_requires_certificate_backed_requirement_by_default() -> None:
    text = SEED_TCC_GUEST.read_text(encoding="utf-8")
    assert '[[ "${ALLOW_ADHOC}" != 1 && "${REQUIREMENT}" != *"certificate leaf"* ]]' in text
    assert "is not signed with a certificate-backed identity" in text


def test_tcc_host_seed_accepts_multiple_vms() -> None:
    text = SEED_TCC.read_text(encoding="utf-8")
    assert 'VMS+=("$1")' in text
    assert 'for vm in "${VMS[@]}"; do' in text
    assert "seed-tcc-guest.sh" in text
    assert "CUA_TCC_READ_SUDO_PASSWORD=1" in text


def test_tcc_host_seed_runs_the_guest_helper_once_per_vm(tmp_path: Path) -> None:
    fake_bin = tmp_path / "bin"
    fake_lume = fake_bin / "lume"
    fake_scp = fake_bin / "scp"
    fake_ssh = fake_bin / "ssh"
    log = tmp_path / "transport.log"
    _write_executable(
        fake_lume,
        """printf 'lume %s\n' "$*" >> "$CUA_TEST_TRANSPORT_LOG"
if [ "$1" = "get" ]; then
  case "$2" in
    worker-a) ip="192.0.2.10" ;;
    worker-b) ip="192.0.2.11" ;;
    *) exit 2 ;;
  esac
  printf '[{"name":"%s","status":"running","sshAvailable":true,"ipAddress":"%s"}]\n' "$2" "$ip"
fi
""",
    )
    _write_executable(
        fake_scp,
        """printf 'scp askpass=%s pass=%s %s\n' "${SSH_ASKPASS_REQUIRE:-}" "${CUA_TCC_SSH_PASSWORD:-}" "$*" >> "$CUA_TEST_TRANSPORT_LOG"
""",
    )
    _write_executable(
        fake_ssh,
        """stdin="$(cat || true)"
printf 'ssh askpass=%s pass=%s stdin=%s %s\n' "${SSH_ASKPASS_REQUIRE:-}" "${CUA_TCC_SSH_PASSWORD:-}" "$stdin" "$*" >> "$CUA_TEST_TRANSPORT_LOG"
""",
    )

    completed = subprocess.run(
        [
            "bash",
            str(SEED_TCC),
            "--timeout",
            "5",
            "worker-a",
            "worker-b",
        ],
        capture_output=True,
        stdin=subprocess.DEVNULL,
        text=True,
        env={
            **os.environ,
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_TEST_TRANSPORT_LOG": str(log),
            "CUA_TCC_SUDO_PASSWORD": "fixture-password",
            "LUME_SSH_USER": "lume",
            "LUME_STORAGE": "",
        },
        check=False,
    )

    assert completed.returncode == 0, completed.stderr
    calls = log.read_text(encoding="utf-8").splitlines()
    assert len(calls) == 10
    assert calls[0] == "lume get worker-a --format json"
    assert calls[1].startswith("scp askpass=force pass=lume -o StrictHostKeyChecking=no ")
    assert "seed-tcc-guest.sh" in calls[1]
    assert calls[1].endswith(" lume@192.0.2.10:/tmp/cua-driver-seed-tcc-guest.sh")
    assert calls[2].endswith(" lume@192.0.2.10 chmod 700 /tmp/cua-driver-seed-tcc-guest.sh")
    assert "stdin=fixture-password" in calls[3]
    assert "CUA_TCC_READ_SUDO_PASSWORD=1 /tmp/cua-driver-seed-tcc-guest.sh" in calls[3]
    assert calls[4].endswith(" lume@192.0.2.10 /bin/rm -f /tmp/cua-driver-seed-tcc-guest.sh")
    assert calls[5] == "lume get worker-b --format json"
    assert calls[6].endswith(" lume@192.0.2.11:/tmp/cua-driver-seed-tcc-guest.sh")
    assert calls[7].endswith(" lume@192.0.2.11 chmod 700 /tmp/cua-driver-seed-tcc-guest.sh")
    assert "stdin=fixture-password" in calls[8]
    assert "CUA_TCC_READ_SUDO_PASSWORD=1 /tmp/cua-driver-seed-tcc-guest.sh" in calls[8]
    assert calls[9].endswith(" lume@192.0.2.11 /bin/rm -f /tmp/cua-driver-seed-tcc-guest.sh")


# --------------------------------------------------------------------------
# Build namespace selection
# --------------------------------------------------------------------------

SHA_A = "1111111111111111111111111111111111111111"
SHA_B = "2222222222222222222222222222222222222222"


def test_target_dir_is_owned_by_source_sha_and_run(tmp_path: Path) -> None:
    env = {"CUA_E2E_CARGO_TARGET_ROOT": str(tmp_path / "cargo-target")}
    first = _run(RUN_ALL, f'resolve_cargo_target_dir "{SHA_A}" "run-1"', env=env)
    second = _run(RUN_ALL, f'resolve_cargo_target_dir "{SHA_A}" "run-2"', env=env)
    other = _run(RUN_ALL, f'resolve_cargo_target_dir "{SHA_B}" "run-1"', env=env)
    assert first.returncode == 0, first.stderr
    assert first.stdout != second.stdout
    assert first.stdout.strip() == str(tmp_path / "cargo-target" / SHA_A / "run-1")
    assert second.stdout.strip() == str(tmp_path / "cargo-target" / SHA_A / "run-2")
    assert other.stdout.strip() == str(tmp_path / "cargo-target" / SHA_B / "run-1")


def test_target_dir_never_lands_in_the_workspace(tmp_path: Path) -> None:
    completed = _run(
        RUN_ALL,
        f'resolve_cargo_target_dir "{SHA_A}" "run-1"',
        env={"CUA_E2E_CARGO_TARGET_ROOT": str(tmp_path / "cargo-target")},
    )
    resolved = Path(completed.stdout.strip())
    assert resolved.is_absolute()
    assert REPO_ROOT not in resolved.parents


def test_consecutive_same_sha_runs_get_distinct_namespaces(tmp_path: Path) -> None:
    env = {"CUA_E2E_CARGO_TARGET_ROOT": str(tmp_path / "cargo-target")}
    first = _run(RUN_ALL, f'resolve_cargo_target_dir "{SHA_A}" "run-1"', env=env)
    second = _run(RUN_ALL, f'resolve_cargo_target_dir "{SHA_A}" "run-2"', env=env)
    assert first.returncode == 0, first.stderr
    assert second.returncode == 0, second.stderr
    assert first.stdout != second.stdout


def test_relative_target_root_is_refused() -> None:
    completed = _run(
        RUN_ALL,
        f'resolve_cargo_target_dir "{SHA_A}" "run-1"',
        env={"CUA_E2E_CARGO_TARGET_ROOT": "relative/target"},
    )
    assert completed.returncode == 2
    assert "absolute path" in completed.stderr


def test_unsafe_run_id_is_refused(tmp_path: Path) -> None:
    completed = _run(
        RUN_ALL,
        f'resolve_cargo_target_dir "{SHA_A}" "../shared"',
        env={"CUA_E2E_CARGO_TARGET_ROOT": str(tmp_path / "cargo-target")},
    )
    assert completed.returncode == 2
    assert "unsafe run id" in completed.stderr


def test_non_sha_build_namespace_is_refused(tmp_path: Path) -> None:
    completed = _run(
        RUN_ALL,
        'resolve_cargo_target_dir "not-a-sha" "run-1"',
        env={"CUA_E2E_CARGO_TARGET_ROOT": str(tmp_path / "cargo-target")},
    )
    assert completed.returncode == 2
    assert "build namespace" in completed.stderr


def test_previous_artifact_run_is_preserved_without_mixing_results(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "macos"
    history_root = tmp_path / "macos-history"
    artifact_dir.mkdir()
    (artifact_dir / "results.jsonl").write_text('{"cell_id":"old"}\n', encoding="utf-8")
    completed = _run(
        RUN_ALL,
        'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        'ARTIFACT_HISTORY_ROOT="$TEST_HISTORY_ROOT"\n'
        'preserve_previous_artifacts "run-2"\n',
        env={
            "TEST_ARTIFACT_DIR": str(artifact_dir),
            "TEST_HISTORY_ROOT": str(history_root),
        },
    )
    assert completed.returncode == 0, completed.stderr
    assert artifact_dir.is_dir()
    assert not any(artifact_dir.iterdir())
    assert (history_root / "run-2/results.jsonl").read_text(encoding="utf-8") == (
        '{"cell_id":"old"}\n'
    )


def test_artifact_history_is_never_overwritten(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "macos"
    history_root = tmp_path / "macos-history"
    artifact_dir.mkdir()
    (artifact_dir / "results.jsonl").write_text("{}\n", encoding="utf-8")
    (history_root / "run-2").mkdir(parents=True)
    completed = _run(
        RUN_ALL,
        'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        'ARTIFACT_HISTORY_ROOT="$TEST_HISTORY_ROOT"\n'
        'preserve_previous_artifacts "run-2"\n',
        env={
            "TEST_ARTIFACT_DIR": str(artifact_dir),
            "TEST_HISTORY_ROOT": str(history_root),
        },
    )
    assert completed.returncode == 2
    assert "refusing to overwrite" in completed.stderr
    assert (artifact_dir / "results.jsonl").is_file()


def test_run_rust_e2e_reads_binaries_from_the_configured_target_dir(tmp_path: Path) -> None:
    rust_root = REPO_ROOT / "libs/cua-driver/rust"
    default = _run(RUN_RUST_E2E, "resolved_build_target_dir", env={"CARGO_TARGET_DIR": ""})
    absolute = _run(
        RUN_RUST_E2E, "resolved_build_target_dir", env={"CARGO_TARGET_DIR": str(tmp_path / "t")}
    )
    relative = _run(RUN_RUST_E2E, "resolved_build_target_dir", env={"CARGO_TARGET_DIR": "nested"})
    assert default.stdout.strip() == str(rust_root / "target")
    assert absolute.stdout.strip() == str(tmp_path / "t")
    assert relative.returncode == 2
    assert "must be absolute" in relative.stderr
    assert 'CUA_TEST_DRIVER_BIN="${BUILD_TARGET_DIR}/release/cua-driver"' in RUN_RUST_E2E.read_text(
        encoding="utf-8"
    )


def test_fixture_builds_use_fresh_run_owned_state() -> None:
    tauri = TAURI_BUILD.read_text(encoding="utf-8")
    electron = ELECTRON_BUILD.read_text(encoding="utf-8")
    assert ELECTRON_LOCK.is_file()
    assert 'tauriTargetDir="${CARGO_TARGET_DIR:-' in tauri
    assert 'srcBin="$tauriTargetDir/release/' in tauri
    assert 'CUA_E2E_FRESH_FIXTURE_STATE:-0}" == 1' in electron
    assert "npm ci --silent" in electron


# --------------------------------------------------------------------------
# Option validation
# --------------------------------------------------------------------------

REPORT_OPTIONS = (
    "status=0\n"
    'parse_arguments "$@" || status=$?\n'
    'printf "status=%s\\n" "$status"\n'
    'printf "cell=%s\\n" "$RETRY_CELL"\n'
    'printf "harness=%s\\n" "$RETRY_HARNESS"\n'
    'printf "attempts=%s\\n" "$RETRY_ATTEMPTS"\n'
    'printf "only=%s\\n" "$RETRY_ONLY"\n'
    'printf "standalone=%s\\n" "$RUN_STANDALONE_BROWSER"\n'
    'printf "nobuild=%s\\n" "$NO_BUILD"\n'
    'printf "lane=%s\\n" "$RETRY_INTERNAL_LANE"\n'
)


def _parse(args: list[str]) -> dict[str, str]:
    completed = _run(RUN_ALL, REPORT_OPTIONS, args=args)
    assert completed.returncode == 0, completed.stderr
    return _fields(completed.stdout)


def test_default_invocation_runs_the_full_matrix() -> None:
    fields = _parse([])
    assert fields["status"] == "0"
    assert fields["cell"] == ""
    assert fields["only"] == "0"
    assert fields["standalone"] == "0"
    assert fields["nobuild"] == "0"


def test_full_retry_selection_is_accepted() -> None:
    fields = _parse(
        [
            "--retry-cell",
            "macos-electron-drag-px-foreground",
            "--retry-harness",
            "electron",
            "--retry-attempts",
            "3",
            "--retry-only",
        ]
    )
    assert fields["status"] == "0"
    assert fields["cell"] == "macos-electron-drag-px-foreground"
    assert fields["harness"] == "electron"
    assert fields["attempts"] == "3"
    assert fields["only"] == "1"


def test_retry_attempts_default_to_one() -> None:
    fields = _parse(["--retry-cell=macos-electron-drag-px-foreground"])
    assert fields["status"] == "0"
    assert fields["attempts"] == "1"


def test_swiftui_retry_is_routed_to_the_native_lane() -> None:
    fields = _parse(
        [
            "--retry-cell",
            "macos-swiftui-left-click-ax-background",
            "--retry-only",
        ]
    )
    assert fields["status"] == "0"
    assert fields["harness"] == "swiftui"
    assert fields["lane"] == "native"


@pytest.mark.parametrize(
    "args",
    [
        [
            "--retry-cell",
            "macos-swiftui-left-click-ax-background",
            "--retry-harness",
            "electron",
        ],
        [
            "--retry-cell",
            "macos-electron-left-click-ax-background",
            "--retry-harness",
            "swiftui",
        ],
    ],
)
def test_swiftui_retry_cell_and_harness_must_agree(args: list[str]) -> None:
    assert _parse(args)["status"] == "2"


def test_native_swiftui_selector_matches_exactly_one_owned_cell() -> None:
    completed = _run(
        RUN_RUST_E2E,
        'CUA_E2E_HARNESS_FILTER="swiftui"\n'
        'CUA_E2E_CELL_FILTER="macos-swiftui-left-click-ax-background"\n'
        'if native_swiftui_test_selected "macos-swiftui-left-click-ax-background"; then '
        'echo "selected=yes"; fi\n'
        'if native_swiftui_test_selected "macos-swiftui-set-value-ax-background"; then '
        'echo "wrong=yes"; fi\n',
    )
    assert completed.returncode == 0, completed.stderr
    assert "selected=yes" in completed.stdout
    assert "wrong=yes" not in completed.stdout


@pytest.mark.parametrize(
    "args",
    [
        pytest.param(["--retry-harness", "electron"], id="harness-without-cell"),
        pytest.param(["--retry-attempts", "2"], id="attempts-without-cell"),
        pytest.param(["--retry-only"], id="retry-only-without-cell"),
        pytest.param(["--retry-cell", "a,b"], id="cell-list"),
        pytest.param(["--retry-cell", "../escape"], id="cell-path-traversal"),
        pytest.param(["--retry-cell", "cell", "--retry-attempts", "0"], id="zero-attempts"),
        pytest.param(["--retry-cell", "cell", "--retry-attempts", "4"], id="too-many-attempts"),
        pytest.param(
            ["--retry-cell", "cell", "--retry-attempts", "two"], id="non-numeric-attempts"
        ),
        pytest.param(["--retry-cell", "cell", "--retry-harness", "Electron!"], id="bad-harness"),
        pytest.param(["--retry-cell"], id="missing-cell-value"),
        pytest.param(["--retry-cell", "--retry-only"], id="flag-as-cell-value"),
        pytest.param(
            ["--retry-cell", "cell", "--retry-only", "--standalone-browser"],
            id="retry-only-with-standalone-browser",
        ),
        pytest.param(["--unknown"], id="unknown-argument"),
    ],
)
def test_invalid_options_are_refused(args: list[str]) -> None:
    fields = _parse(args)
    assert fields["status"] == "2"


def test_help_exits_without_running() -> None:
    completed = _run(RUN_ALL, REPORT_OPTIONS, args=["--help"])
    fields = _fields(completed.stdout)
    assert fields["status"] == "3"
    assert "--retry-cell" in completed.stdout


def test_full_matrix_clears_inherited_retry_filters(tmp_path: Path) -> None:
    fake_repo = tmp_path / "repo"
    output = tmp_path / "filters.txt"
    _write_executable(
        fake_repo / "scripts/ci/macos/run-rust-e2e.sh",
        """printf 'cell=%s\nharness=%s\nlane=%s\n' \\
    "${CUA_E2E_CELL_FILTER:-}" "${CUA_E2E_HARNESS_FILTER:-}" \\
    "${CUA_E2E_INTERNAL_LANE:-}" > "$CUA_TEST_FILTER_OUTPUT"
""",
    )
    completed = _run(
        RUN_ALL,
        'REPO_ROOT="$TEST_FAKE_REPO"\nrun_full_matrix\n',
        env={
            "TEST_FAKE_REPO": str(fake_repo),
            "CUA_TEST_FILTER_OUTPUT": str(output),
            "CUA_E2E_CELL_FILTER": "stale-cell",
            "CUA_E2E_HARNESS_FILTER": "electron",
            "CUA_E2E_INTERNAL_LANE": "shared",
        },
    )
    assert completed.returncode == 0, completed.stderr
    assert output.read_text(encoding="utf-8") == "cell=\nharness=\nlane=\n"


def test_unrestricted_daemon_admits_the_history_preview(tmp_path: Path) -> None:
    fake_open = tmp_path / "bin/open"
    output = tmp_path / "open-args.txt"
    _write_executable(fake_open, 'printf "%s\\n" "$@" > "$CUA_TEST_OPEN_ARGS"\n')
    completed = _run(
        RUN_ALL,
        'LOCAL_APP="/Applications/Test.app"\nstart_unrestricted_daemon\n',
        env={
            "PATH": f"{fake_open.parent}:{os.environ['PATH']}",
            "CUA_TEST_OPEN_ARGS": str(output),
        },
    )
    assert completed.returncode == 0, completed.stderr
    assert output.read_text(encoding="utf-8").splitlines() == [
        "-n",
        "-g",
        "/Applications/Test.app",
        "--args",
        "serve",
        "--permission-mode",
        "unrestricted",
        "--dangerously-bypass-approvals",
        "--experimental-history",
    ]


def test_history_hook_p99_parser_is_exact(tmp_path: Path) -> None:
    report = tmp_path / "history-hook-benchmark.txt"
    report.write_text(
        "accepted: p50=10ns p95=20ns p99=30ns n=2000\n"
        "full_queue: p50=4ns p95=5ns p99=6ns n=20000\n",
        encoding="utf-8",
    )
    completed = _run(
        RUN_ALL,
        'printf "accepted=%s\\n" "$(history_hook_p99_ns accepted "$REPORT")"\n'
        'printf "full=%s\\n" "$(history_hook_p99_ns full_queue "$REPORT")"\n',
        env={"REPORT": str(report)},
    )
    assert completed.returncode == 0, completed.stderr
    assert _fields(completed.stdout) == {"accepted": "30", "full": "6"}


def test_runner_requires_an_exact_identity_hash_when_configured() -> None:
    text = RUN_ALL.read_text(encoding="utf-8")
    assert '[[ ! "${CUA_E2E_SIGNING_IDENTITY}" =~ ^[0-9A-Fa-f]{40}$ ]]' in text
    assert 'export CUA_DRIVER_LOCAL_SIGNING_IDENTITY="${CUA_E2E_SIGNING_IDENTITY}"' in text


def test_required_keychains_unlock_login_without_retaining_password(
    tmp_path: Path,
) -> None:
    signing_keychain = tmp_path / "signing.keychain-db"
    login_keychain = tmp_path / "login.keychain-db"
    signing_keychain.touch()
    login_keychain.touch()
    fake_security = tmp_path / "bin/security"
    log = tmp_path / "security.log"
    _write_executable(
        fake_security,
        """printf '%s\\n' "$*" >> "$CUA_TEST_SECURITY_LOG"
""",
    )
    completed = _run(
        RUN_ALL,
        "unlock_required_keychains\n"
        'printf "password_present=%s\\n" "${CUA_E2E_SIGNING_KEYCHAIN_PASSWORD+x}"\n',
        env={
            "PATH": f"{fake_security.parent}:{os.environ['PATH']}",
            "CUA_E2E_SIGNING_KEYCHAIN": str(signing_keychain),
            "CUA_E2E_LOGIN_KEYCHAIN": str(login_keychain),
            "CUA_E2E_SIGNING_KEYCHAIN_PASSWORD": "fixture-password",
            "CUA_TEST_SECURITY_LOG": str(log),
        },
    )
    assert completed.returncode == 0, completed.stderr
    assert "password_present=" in completed.stdout
    assert log.read_text(encoding="utf-8").splitlines() == [
        f"unlock-keychain -p fixture-password {signing_keychain}",
        f"unlock-keychain -p fixture-password {login_keychain}",
    ]


# --------------------------------------------------------------------------
# Retry bookkeeping
# --------------------------------------------------------------------------


def _result_row(cell_id: str, status: str, harness: str = "electron") -> dict[str, object]:
    return {
        "schema": "cua-driver/e2e-result@v1",
        "cell_id": cell_id,
        "harness": harness,
        "test_status": status,
    }


def _write_results(path: Path, rows: list[dict[str, object]]) -> None:
    path.write_text("".join(f"{json.dumps(row)}\n" for row in rows), encoding="utf-8")


def _write_failures(path: Path, **overrides: object) -> None:
    record: dict[str, object] = {
        "schema": "cua-driver/e2e-failures@v1",
        "suite": "all",
        "filters": {"cell": "", "harness": ""},
        "failure_count": 1,
        "preflight_failed": False,
        "report_failed": False,
        "lanes": ["shared-app-matrix"],
        "video_failures": [],
    }
    record.update(overrides)
    path.write_text(json.dumps(record), encoding="utf-8")


CHECK_ELIGIBILITY = (
    'RETRY_CELL="$RETRY_CELL_UNDER_TEST"\n'
    'RETRY_HARNESS="$RETRY_HARNESS_UNDER_TEST"\n'
    "status=0\n"
    'retry_selection_is_eligible "$RESULTS" "$FAILURES" || status=$?\n'
    'printf "status=%s\\n" "$status"\n'
    'printf "cells=%s\\n" "${RETRY_INITIAL_CELLS[*]:-}"\n'
    'printf "lanes=%s\\n" "$RETRY_INITIAL_LANES_JSON"\n'
)


def _eligibility(
    tmp_path: Path,
    rows: list[dict[str, object]],
    cell: str = "macos-electron-drag-px-foreground",
    harness: str = "",
    failures: dict[str, object] | None = None,
    write_failures: bool = True,
) -> dict[str, str]:
    results = tmp_path / "results.jsonl"
    failures_file = tmp_path / "failures.json"
    _write_results(results, rows)
    if write_failures:
        _write_failures(failures_file, **(failures or {}))
    completed = _run(
        RUN_ALL,
        CHECK_ELIGIBILITY,
        env={
            "RESULTS": str(results),
            "FAILURES": str(failures_file),
            "RETRY_CELL_UNDER_TEST": cell,
            "RETRY_HARNESS_UNDER_TEST": harness,
        },
    )
    assert completed.returncode == 0, completed.stderr
    fields = _fields(completed.stdout)
    fields["stderr"] = completed.stderr
    return fields


@requires_jq
def test_single_cell_failure_is_retryable(tmp_path: Path) -> None:
    fields = _eligibility(
        tmp_path,
        [
            _result_row("macos-electron-click-ax-background", "pass"),
            _result_row("macos-electron-drag-px-foreground", "fail"),
        ],
        harness="electron",
    )
    assert fields["status"] == "0"
    assert fields["cells"] == "macos-electron-drag-px-foreground"
    assert fields["lanes"] == '["shared-app-matrix"]'


@requires_jq
def test_two_failing_cells_are_not_retryable(tmp_path: Path) -> None:
    fields = _eligibility(
        tmp_path,
        [
            _result_row("macos-electron-drag-px-foreground", "fail"),
            _result_row("macos-tauri-scroll-px-background", "fail"),
        ],
    )
    assert fields["status"] == "1"
    assert "are not exactly" in fields["stderr"]


@requires_jq
def test_a_different_failing_cell_is_not_retryable(tmp_path: Path) -> None:
    fields = _eligibility(tmp_path, [_result_row("macos-tauri-drag-px-foreground", "fail")])
    assert fields["status"] == "1"


@requires_jq
def test_a_skipped_cell_counts_as_a_failure(tmp_path: Path) -> None:
    fields = _eligibility(
        tmp_path,
        [
            _result_row("macos-electron-drag-px-foreground", "fail"),
            _result_row("macos-electron-click-ax-background", "skip"),
        ],
    )
    assert fields["status"] == "1"


@requires_jq
def test_harness_mismatch_is_not_retryable(tmp_path: Path) -> None:
    fields = _eligibility(
        tmp_path,
        [_result_row("macos-electron-drag-px-foreground", "fail", harness="wkwebview")],
        harness="electron",
    )
    assert fields["status"] == "1"
    assert "belongs to harness" in fields["stderr"]


@requires_jq
@pytest.mark.parametrize(
    ("failures", "expected"),
    [
        pytest.param({"preflight_failed": True}, "preflight", id="preflight"),
        pytest.param(
            {"report_failed": True, "failure_count": 2},
            "report validation",
            id="report-validation",
        ),
        pytest.param({"video_failures": ["orphan:recordings/x"]}, "video", id="video"),
        pytest.param({"failure_count": 2}, "failure signals", id="extra-failure-signal"),
        pytest.param(
            {"lanes": ["shared-app-matrix", "capture-contract"]}, "lanes failed", id="two-lanes"
        ),
        pytest.param({"lanes": ["capture-contract"]}, "not retryable", id="non-retryable-lane"),
        pytest.param(
            {"lanes": ["embedded-browser-routes"]}, "not retryable", id="embedded-browser-lane"
        ),
        pytest.param({"lanes": []}, "lanes failed", id="no-lane"),
    ],
)
def test_non_cell_failures_are_never_retryable(
    tmp_path: Path, failures: dict[str, object], expected: str
) -> None:
    fields = _eligibility(
        tmp_path, [_result_row("macos-electron-drag-px-foreground", "fail")], failures=failures
    )
    assert fields["status"] == "1"
    assert expected in fields["stderr"]


@requires_jq
def test_missing_failure_record_blocks_the_retry(tmp_path: Path) -> None:
    fields = _eligibility(
        tmp_path,
        [_result_row("macos-electron-drag-px-foreground", "fail")],
        write_failures=False,
    )
    assert fields["status"] == "1"
    assert "machine-readable failure record" in fields["stderr"]


@requires_jq
def test_missing_results_block_the_retry(tmp_path: Path) -> None:
    completed = _run(
        RUN_ALL,
        CHECK_ELIGIBILITY,
        env={
            "RESULTS": str(tmp_path / "absent.jsonl"),
            "FAILURES": str(tmp_path / "absent.json"),
            "RETRY_CELL_UNDER_TEST": "macos-electron-drag-px-foreground",
            "RETRY_HARNESS_UNDER_TEST": "",
        },
    )
    assert _fields(completed.stdout)["status"] == "1"
    assert "no typed result rows" in completed.stderr


@requires_jq
def test_retry_attempt_records_keep_every_attempt(tmp_path: Path) -> None:
    completed = _run(RUN_ALL, "RETRY_EXIT_CODES=(1 1 0)\nretry_attempts_json")
    assert completed.returncode == 0, completed.stderr
    attempts = json.loads(completed.stdout)
    assert [attempt["status"] for attempt in attempts] == ["fail", "fail", "pass"]
    assert [attempt["evidence"] for attempt in attempts] == [
        "attempts/retry-1",
        "attempts/retry-2",
        ".",
    ]
    empty = _run(RUN_ALL, "RETRY_EXIT_CODES=()\nretry_attempts_json")
    assert json.loads(empty.stdout) == []


@requires_jq
def test_retry_result_must_be_exactly_one_selected_cell(tmp_path: Path) -> None:
    results = tmp_path / "results.jsonl"
    selected = "macos-electron-drag-px-foreground"
    _write_results(results, [_result_row(selected, "pass")])
    completed = _run(
        RUN_ALL,
        'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        'RETRY_CELL="$TEST_RETRY_CELL"\n'
        "status=0\nretry_result_is_exact || status=$?\n"
        'printf "status=%s\\n" "$status"\n',
        env={"TEST_ARTIFACT_DIR": str(tmp_path), "TEST_RETRY_CELL": selected},
    )
    assert _fields(completed.stdout)["status"] == "0"

    _write_results(
        results,
        [
            _result_row(selected, "pass"),
            _result_row(f"{selected}-extra", "pass"),
        ],
    )
    completed = _run(
        RUN_ALL,
        'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        'RETRY_CELL="$TEST_RETRY_CELL"\n'
        "status=0\nretry_result_is_exact || status=$?\n"
        'printf "status=%s\\n" "$status"\n',
        env={"TEST_ARTIFACT_DIR": str(tmp_path), "TEST_RETRY_CELL": selected},
    )
    assert _fields(completed.stdout)["status"] == "1"


@requires_jq
def test_retry_record_keeps_the_initial_failure(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    (artifact_dir / "summary.md").write_text("# macOS E2E\n\nall green\n", encoding="utf-8")
    initial = json.dumps(
        {
            "status": "fail",
            "exit_code": 1,
            "failing_cells": ["macos-electron-drag-px-foreground"],
            "lanes": ["shared-app-matrix"],
            "evidence": "attempts/attempt-1-full-matrix",
        }
    )
    completed = _run(
        RUN_ALL,
        'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        'RETRY_CELL="macos-electron-drag-px-foreground"\n'
        'RETRY_HARNESS="electron"\n'
        "RETRY_ATTEMPTS=2\n"
        "RETRY_EXIT_CODES=(1 0)\n"
        'write_retry_record pass-after-retry "$INITIAL_JSON"\n',
        env={"TEST_ARTIFACT_DIR": str(artifact_dir), "INITIAL_JSON": initial},
    )
    assert completed.returncode == 0, completed.stderr

    record = json.loads((artifact_dir / "retry-record.json").read_text(encoding="utf-8"))
    assert record["schema"] == "cua-driver/e2e-retry-record@v1"
    assert record["mode"] == "after-full-matrix"
    assert record["final_status"] == "pass-after-retry"
    assert record["selection"] == {
        "cell_id": "macos-electron-drag-px-foreground",
        "harness": "electron",
        "internal_lane": "shared",
        "allowed_attempts": 2,
    }
    assert record["initial_attempt"]["status"] == "fail"
    assert record["initial_attempt"]["failing_cells"] == ["macos-electron-drag-px-foreground"]
    assert [attempt["status"] for attempt in record["retry_attempts"]] == ["fail", "pass"]

    summary = (artifact_dir / "summary.md").read_text(encoding="utf-8")
    assert "all green" in summary
    assert "Retry provenance" in summary
    assert "Initial full matrix: FAILED on `macos-electron-drag-px-foreground`" in summary
    assert "#1 fail" in summary and "#2 pass" in summary


@requires_jq
def test_retry_only_record_states_that_no_full_matrix_ran(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    completed = _run(
        RUN_ALL,
        'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        'RETRY_CELL="macos-electron-drag-px-foreground"\n'
        "RETRY_ONLY=1\n"
        "RETRY_ATTEMPTS=1\n"
        "RETRY_EXIT_CODES=(1)\n"
        "write_retry_record fail-retry-only null\n",
        env={"TEST_ARTIFACT_DIR": str(artifact_dir)},
    )
    assert completed.returncode == 0, completed.stderr
    record = json.loads((artifact_dir / "retry-record.json").read_text(encoding="utf-8"))
    assert record["mode"] == "retry-only"
    assert record["initial_attempt"] is None
    assert record["final_status"] == "fail-retry-only"
    summary = (artifact_dir / "summary.md").read_text(encoding="utf-8")
    assert "not run in this invocation" in summary


def test_archiving_evidence_preserves_the_previous_attempt(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    (artifact_dir / "recordings/cell-a").mkdir(parents=True)
    (artifact_dir / "recordings/cell-a/recording.mp4").write_text("mp4", encoding="utf-8")
    (artifact_dir / "results.jsonl").write_text("{}\n", encoding="utf-8")
    (artifact_dir / "cases.jsonl").write_text("{}\n", encoding="utf-8")
    (artifact_dir / "summary.md").write_text("summary\n", encoding="utf-8")
    (artifact_dir / "shared-app-matrix.log").write_text("log\n", encoding="utf-8")
    (artifact_dir / "failures.json").write_text("{}\n", encoding="utf-8")

    completed = _run(
        RUN_ALL,
        'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        "archive_attempt_evidence attempt-1-full-matrix\n"
        'printf "path=%s\\n" "$ATTEMPT_EVIDENCE_PATH"\n',
        env={"TEST_ARTIFACT_DIR": str(artifact_dir)},
    )
    assert completed.returncode == 0, completed.stderr
    assert _fields(completed.stdout)["path"] == "attempts/attempt-1-full-matrix"

    archived = artifact_dir / "attempts/attempt-1-full-matrix"
    assert (archived / "recordings/cell-a/recording.mp4").read_text(encoding="utf-8") == "mp4"
    copied = (
        "results.jsonl",
        "cases.jsonl",
        "summary.md",
        "shared-app-matrix.log",
        "failures.json",
    )
    for name in copied:
        assert (archived / name).exists(), name
        # Copies keep the root readable; nothing is deleted to make room.
        assert (artifact_dir / name).exists(), name
    # Only the recordings directory moves, so the next attempt starts empty.
    assert not (artifact_dir / "recordings").exists()


# --------------------------------------------------------------------------
# Daemon ownership and restoration
# --------------------------------------------------------------------------


def _daemon_fakes(tmp_path: Path, initial_mode: str = "standard") -> tuple[Path, dict[str, str]]:
    """Fake driver CLI, launchctl, and open that record what the runner did."""
    fake_bin = tmp_path / "fake-bin"
    mode_file = tmp_path / "mode"
    mode_file.write_text(f"{initial_mode}\n", encoding="utf-8")
    calls = tmp_path / "calls.txt"

    _write_executable(
        fake_bin / "cua-driver-local",
        """case "${1:-}" in
    status) printf 'permission mode: %s\n' "$(cat "$CUA_TEST_MODE_FILE")" ;;
    stop) printf 'stop\n' >> "$CUA_TEST_CALLS"; printf 'stopped\n' > "$CUA_TEST_MODE_FILE" ;;
    *) printf 'unsupported %s\n' "${1:-}" >&2; exit 2 ;;
esac
exit 0
""",
    )
    _write_executable(
        fake_bin / "launchctl",
        """printf 'launchctl %s\n' "$*" >> "$CUA_TEST_CALLS"
if [ "${1:-}" = print ] && [ "${CUA_TEST_GUI_UNAVAILABLE:-0}" = 1 ]; then
    exit 125
fi
if [ "${1:-}" = load ]; then
    printf '%s\n' "${CUA_TEST_LOAD_MODE:-standard}" > "$CUA_TEST_MODE_FILE"
fi
exit 0
""",
    )
    _write_executable(
        fake_bin / "open",
        """printf 'open %s\n' "$*" >> "$CUA_TEST_CALLS"
printf '%s\n' "${CUA_TEST_START_MODE:-unrestricted}" > "$CUA_TEST_MODE_FILE"
exit 0
""",
    )
    env = {
        "PATH": f"{fake_bin}:{os.environ['PATH']}",
        "TEST_INSTALLED_BIN": str(fake_bin / "cua-driver-local"),
        "CUA_TEST_MODE_FILE": str(mode_file),
        "CUA_TEST_CALLS": str(calls),
        "CUA_E2E_DAEMON_WAIT_ATTEMPTS": "1",
    }
    return calls, env


def test_ensure_unrestricted_daemon_starts_and_verifies_the_mode(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    calls, env = _daemon_fakes(tmp_path)
    env["TEST_ARTIFACT_DIR"] = str(artifact_dir)
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + 'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        "status=0\n"
        "ensure_unrestricted_daemon || status=$?\n"
        "stop_unrestricted_watchdog\n"
        'printf "status=%s\\n" "$status"\n'
        'printf "restore=%s\\n" "$RESTORE_STANDARD_DAEMON"\n',
        env=env,
    )
    assert completed.returncode == 0, completed.stderr
    fields = _fields(completed.stdout)
    assert fields["status"] == "0"
    assert fields["restore"] == "1"
    recorded = calls.read_text(encoding="utf-8")
    assert "launchctl unload" in recorded
    assert "--permission-mode unrestricted" in recorded
    assert "--dangerously-bypass-approvals" in recorded


def test_ensure_unrestricted_daemon_fails_when_the_mode_never_appears(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    _calls, env = _daemon_fakes(tmp_path)
    env["TEST_ARTIFACT_DIR"] = str(artifact_dir)
    env["CUA_TEST_START_MODE"] = "standard"
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + 'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        "status=0\n"
        "ensure_unrestricted_daemon || status=$?\n"
        "stop_unrestricted_watchdog\n"
        'printf "status=%s\\n" "$status"\n',
        env=env,
    )
    assert _fields(completed.stdout)["status"] == "1"
    assert "could not start its unrestricted worker daemon" in completed.stderr


def test_a_targeted_retry_reclaims_the_daemon_before_running(tmp_path: Path) -> None:
    """A retry must not inherit whatever mode the worker happens to be in."""
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    calls, env = _daemon_fakes(tmp_path)
    env["TEST_ARTIFACT_DIR"] = str(artifact_dir)
    # A stub matrix keeps this test on the daemon and filter contract.
    fake_repo = tmp_path / "fake-repo"
    _write_executable(
        fake_repo / "scripts/ci/macos/run-rust-e2e.sh",
        """printf 'matrix lane=%s cell=%s harness=%s args=[%s]\n' \\
    "$CUA_E2E_INTERNAL_LANE" "$CUA_E2E_CELL_FILTER" "$CUA_E2E_HARNESS_FILTER" "$*" \\
    >> "$CUA_TEST_CALLS"
printf '{"cell_id":"%s","test_status":"pass"}\n' "$CUA_E2E_CELL_FILTER" \\
    > "$CUA_TEST_RESULTS_FILE"
exit 0
""",
    )
    env["TEST_FAKE_REPO"] = str(fake_repo)
    env["CUA_TEST_RESULTS_FILE"] = str(artifact_dir / "results.jsonl")
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + 'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        'REPO_ROOT="$TEST_FAKE_REPO"\n'
        'RETRY_CELL="macos-electron-drag-px-foreground"\n'
        'RETRY_HARNESS="electron"\n'
        "RETRY_ATTEMPTS=1\n"
        "status=0\n"
        "run_retry_sequence || status=$?\n"
        "stop_unrestricted_watchdog\n"
        'printf "status=%s\\n" "$status"\n',
        env=env,
    )
    assert completed.returncode == 0, completed.stderr
    assert _fields(completed.stdout)["status"] == "0"
    recorded = calls.read_text(encoding="utf-8")
    assert "--permission-mode unrestricted" in recorded
    assert (
        "matrix lane=shared cell=macos-electron-drag-px-foreground harness=electron args=[]"
        in recorded
    )
    # The daemon is claimed and verified before the retry runs.
    assert recorded.index("--permission-mode unrestricted") < recorded.index("matrix lane=")


def test_a_persistent_failure_exhausts_the_bounded_attempts(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    calls, env = _daemon_fakes(tmp_path)
    env["TEST_ARTIFACT_DIR"] = str(artifact_dir)
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + 'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        'RETRY_CELL="macos-electron-drag-px-foreground"\n'
        "RETRY_ATTEMPTS=3\n"
        'run_retry_matrix() { printf "attempt\\n" >> "$CUA_TEST_CALLS"; return 1; }\n'
        "status=0\n"
        "run_retry_sequence || status=$?\n"
        "stop_unrestricted_watchdog\n"
        'printf "status=%s\\n" "$status"\n'
        'printf "codes=%s\\n" "${RETRY_EXIT_CODES[*]}"\n',
        env=env,
    )
    fields = _fields(completed.stdout)
    assert fields["status"] == "1", completed.stderr
    assert fields["codes"] == "1 1 1"
    assert calls.read_text(encoding="utf-8").count("attempt\n") == 3


def test_retry_records_a_daemon_start_failure_as_an_attempt() -> None:
    completed = _run(
        RUN_ALL,
        'RETRY_CELL="macos-electron-drag-px-foreground"\n'
        "RETRY_ATTEMPTS=2\n"
        "ensure_unrestricted_daemon() { return 1; }\n"
        "status=0\n"
        "run_retry_sequence || status=$?\n"
        'printf "status=%s\\n" "$status"\n'
        'printf "codes=%s\\n" "${RETRY_EXIT_CODES[*]}"\n',
    )
    fields = _fields(completed.stdout)
    assert fields["status"] == "1"
    assert fields["codes"] == "1"


def test_watchdog_does_not_restart_after_one_transient_probe_failure(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    calls, env = _daemon_fakes(tmp_path, initial_mode="unrestricted")
    probe_count = tmp_path / "probe-count"
    probe_count.write_text("0\n", encoding="utf-8")
    env["CUA_TEST_PROBE_COUNT"] = str(probe_count)
    env["TEST_ARTIFACT_DIR"] = str(artifact_dir)
    _write_executable(
        Path(env["TEST_INSTALLED_BIN"]),
        """case "${1:-}" in
    status)
        count=$(cat "$CUA_TEST_PROBE_COUNT")
        count=$((count + 1))
        printf '%s\n' "$count" > "$CUA_TEST_PROBE_COUNT"
        if [ "$count" -eq 1 ]; then exit 3; fi
        printf 'permission mode: unrestricted\n'
        ;;
    stop) printf 'stop\n' >> "$CUA_TEST_CALLS" ;;
esac
exit 0
""",
    )
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + 'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\nwatchdog_check_once\n',
        env=env,
    )
    assert completed.returncode == 0, completed.stderr
    assert not calls.exists() or "stop" not in calls.read_text(encoding="utf-8")
    assert probe_count.read_text(encoding="utf-8").strip() == "2"


@pytest.mark.parametrize("body_status", [0, 7], ids=["success", "failure"])
def test_the_trap_restores_the_standard_daemon_and_keeps_the_status(
    tmp_path: Path, body_status: int
) -> None:
    calls, env = _daemon_fakes(tmp_path, initial_mode="unrestricted")
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + "RESTORE_STANDARD_DAEMON=1\n"
        "install_daemon_restore_traps\n"
        'exit "$TEST_BODY_STATUS"\n',
        env={**env, "TEST_BODY_STATUS": str(body_status)},
    )
    assert completed.returncode == body_status, completed.stderr
    recorded = calls.read_text(encoding="utf-8")
    assert "stop" in recorded
    assert "launchctl load" in recorded


def test_sigterm_restores_standard_daemon_and_preserves_signal_status(tmp_path: Path) -> None:
    calls, env = _daemon_fakes(tmp_path, initial_mode="unrestricted")
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + "RESTORE_STANDARD_DAEMON=1\n"
        "install_daemon_restore_traps\n"
        "kill -TERM $$\n",
        env=env,
    )
    assert completed.returncode == 143, completed.stderr
    recorded = calls.read_text(encoding="utf-8")
    assert "stop" in recorded
    assert "launchctl load" in recorded


def test_sighup_defers_standard_daemon_until_the_next_gui_login(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    plist = tmp_path / "com.trycua.cua-driver-local.plist"
    plist.write_text("launch agent", encoding="utf-8")
    calls, env = _daemon_fakes(tmp_path, initial_mode="unrestricted")
    env.update(
        {
            "CUA_TEST_GUI_UNAVAILABLE": "1",
            "TEST_ARTIFACT_DIR": str(artifact_dir),
            "TEST_LOCAL_PLIST": str(plist),
        }
    )
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE
        + 'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\n'
        + 'LOCAL_PLIST="$TEST_LOCAL_PLIST"\n'
        + "RESTORE_STANDARD_DAEMON=1\n"
        + "install_daemon_restore_traps\n"
        + "kill -HUP $$\n",
        env=env,
    )
    assert completed.returncode == 129, completed.stderr
    recorded = calls.read_text(encoding="utf-8")
    assert "launchctl print" in recorded
    assert "launchctl load" not in recorded
    marker = artifact_dir / "standard-daemon-restore-deferred.txt"
    assert marker.read_text(encoding="utf-8") == (
        "status=deferred-until-next-gui-login\nexit_status=129\n"
    )


def test_a_failed_restoration_fails_an_otherwise_green_run(tmp_path: Path) -> None:
    _calls, env = _daemon_fakes(tmp_path, initial_mode="unrestricted")
    env["CUA_TEST_LOAD_MODE"] = "unrestricted"
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + "RESTORE_STANDARD_DAEMON=1\ninstall_daemon_restore_traps\nexit 0\n",
        env=env,
    )
    assert completed.returncode == 1
    assert "could not restore its standard autostart daemon" in completed.stderr


def test_the_trap_leaves_an_unclaimed_daemon_alone(tmp_path: Path) -> None:
    calls, env = _daemon_fakes(tmp_path)
    completed = _run(
        RUN_ALL,
        INSTALLED_BIN_PRELUDE + "RESTORE_STANDARD_DAEMON=0\ninstall_daemon_restore_traps\nexit 4\n",
        env=env,
    )
    assert completed.returncode == 4
    assert not calls.exists() or "launchctl load" not in calls.read_text(encoding="utf-8")


# --------------------------------------------------------------------------
# Machine-readable failure record produced by the matrix runner
# --------------------------------------------------------------------------


@requires_jq
def test_failure_record_separates_cells_lanes_videos_and_preflight(tmp_path: Path) -> None:
    record_file = tmp_path / "failures.json"
    completed = _run(
        RUN_RUST_E2E,
        'SUITE=shared\nCUA_E2E_CELL_FILTER="macos-electron-drag-px-foreground"\n'
        'CUA_E2E_HARNESS_FILTER="electron"\n'
        "note_lane_failure shared-app-matrix\n"
        "note_video_failure orphan:recordings/x/recording.mp4\n"
        "REPORT_FAILED=1\n"
        "FAILURE_COUNT=$((FAILURE_COUNT + 1))\n"
        'write_failure_record "$RECORD_FILE"\n',
        env={"RECORD_FILE": str(record_file)},
    )
    assert completed.returncode == 0, completed.stderr
    record = json.loads(record_file.read_text(encoding="utf-8"))
    assert record["schema"] == "cua-driver/e2e-failures@v1"
    assert record["suite"] == "shared"
    assert record["filters"] == {
        "cell": "macos-electron-drag-px-foreground",
        "harness": "electron",
    }
    assert record["lanes"] == ["shared-app-matrix"]
    assert record["video_failures"] == ["orphan:recordings/x/recording.mp4"]
    assert record["preflight_failed"] is False
    assert record["report_failed"] is True
    assert record["failure_count"] == 3


@requires_jq
def test_a_green_run_writes_an_empty_failure_record(tmp_path: Path) -> None:
    record_file = tmp_path / "failures.json"
    completed = _run(
        RUN_RUST_E2E,
        'write_failure_record "$RECORD_FILE"',
        env={"RECORD_FILE": str(record_file)},
    )
    assert completed.returncode == 0, completed.stderr
    record = json.loads(record_file.read_text(encoding="utf-8"))
    assert record["failure_count"] == 0
    assert record["lanes"] == []
    assert record["video_failures"] == []
    assert record["preflight_failed"] is False
    assert record["report_failed"] is False


def test_lane_bookkeeping_records_only_failing_lanes(tmp_path: Path) -> None:
    artifact_dir = tmp_path / "artifacts"
    artifact_dir.mkdir()
    completed = _run(
        RUN_RUST_E2E,
        'ARTIFACT_DIR="$TEST_ARTIFACT_DIR"\nRUST_ROOT="$TEST_ARTIFACT_DIR"\n'
        "run_test green true\n"
        "run_test red false\n"
        'printf "count=%s\\n" "$FAILURE_COUNT"\n'
        'printf "lanes=%s\\n" "${FAILED_LANES[*]}"\n',
        env={"TEST_ARTIFACT_DIR": str(artifact_dir)},
    )
    assert completed.returncode == 0, completed.stderr
    fields = _fields(completed.stdout)
    assert fields["count"] == "1"
    assert fields["lanes"] == "red"
    assert (artifact_dir / "green.log").exists()
    assert (artifact_dir / "red.log").exists()
