"""Regression tests for CARGO_TARGET_DIR handling in scripts/ci/linux/run-rust-e2e.sh."""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
RUNNER_SCRIPT = REPO_ROOT / "scripts/ci/linux/run-rust-e2e.sh"

STUB_CARGO = """#!/usr/bin/env bash
set -euo pipefail

subcmd="${1:-}"
shift || true

manifest_dir=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  if [[ "${args[$i]}" == "--manifest-path" && $((i + 1)) -lt ${#args[@]} ]]; then
    manifest_dir="$(dirname "${args[$((i + 1))]}")"
    break
  fi
done

if [[ -n "${CARGO_TARGET_DIR+x}" ]]; then
  target_state="set"
  target_val="${CARGO_TARGET_DIR}"
else
  target_state="unset"
  target_val=""
fi

python3 - "${STUB_CARGO_LOG}" "${subcmd}" "${PWD}" "${target_state}" "${target_val}" "${CUA_TEST_DRIVER_BIN:-}" <<'PY'
import json
import sys

log_path, subcmd, cwd, target_state, target_val, driver_bin = sys.argv[1:7]
record = {
    "subcmd": subcmd,
    "cwd": cwd,
    "cargo_target_dir": None if target_state == "unset" else target_val,
    "cua_test_driver_bin": driver_bin,
}
with open(log_path, "a", encoding="utf-8") as fh:
    fh.write(json.dumps(record) + "\\n")
PY

if [[ "${target_state}" == "set" && -z "${target_val}" ]]; then
  echo 'error: the target directory is set to an empty string in the `CARGO_TARGET_DIR` environment variable' >&2
  exit 101
fi

case "${subcmd}" in
  build)
    if [[ "${target_state}" == "unset" ]]; then
      base_dir="${manifest_dir:-${PWD}}/target"
    elif [[ "${target_val}" == /* ]]; then
      base_dir="${target_val}"
    else
      # Match Cargo's resolution of a relative CARGO_TARGET_DIR against cwd.
      base_dir="${PWD}/${target_val}"
    fi
    base_dir="${base_dir%/}"
    mkdir -p "${base_dir}/release"
    cat > "${base_dir}/release/cua-driver" <<'BIN'
#!/usr/bin/env bash
exit 0
BIN
    chmod +x "${base_dir}/release/cua-driver"
    exit 0
    ;;
  test)
    # Stop at preflight so no desktop session is needed.
    test -x "${CUA_TEST_DRIVER_BIN:-}"
    echo "PREFLIGHT_DRIVER_OK"
    exit 42
    ;;
  run)
    exit 0
    ;;
  *)
    echo "unexpected stub cargo subcommand: ${subcmd}" >&2
    exit 99
    ;;
esac
"""

STUB_FIXTURE_BUILDER = """#!/usr/bin/env bash
set -euo pipefail

if [[ -n "${CARGO_TARGET_DIR+x}" ]]; then
  printf 'set:%s\\n' "${CARGO_TARGET_DIR}" > "${STUB_FIXTURE_LOG}"
else
  printf 'unset\\n' > "${STUB_FIXTURE_LOG}"
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
test_apps="$(cd "${script_dir}/../../../rust/test-apps" && pwd)"
mkdir -p "${test_apps}/harness-electron"
cat > "${test_apps}/harness-electron/CuaTestHarness.Electron" <<'BIN'
#!/usr/bin/env bash
exit 0
BIN
chmod +x "${test_apps}/harness-electron/CuaTestHarness.Electron"
"""


def _write_executable(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux E2E runner requires Linux bash")
class LinuxRustE2ETargetDirTest(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.repo = self.tmp / "repo"
        self.caller_cwd = self.tmp / "caller-cwd"
        self.caller_cwd.mkdir()
        self.rust_root = self.repo / "libs/cua-driver/rust"
        (self.rust_root / "test-apps").mkdir(parents=True)
        (self.rust_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")

        runner_dest = self.repo / "scripts/ci/linux/run-rust-e2e.sh"
        runner_dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(RUNNER_SCRIPT, runner_dest)
        self.runner = runner_dest

        _write_executable(
            self.repo / "libs/cua-driver/tests/fixtures/build/linux.sh",
            STUB_FIXTURE_BUILDER,
        )

        self.stub_bin = self.tmp / "stub-bin"
        _write_executable(self.stub_bin / "cargo", STUB_CARGO)
        for tool in ("ffmpeg", "ffprobe", "jq"):
            _write_executable(self.stub_bin / tool, "#!/usr/bin/env bash\nexit 0\n")

        self.cargo_log = self.tmp / "cargo-calls.jsonl"
        self.fixture_log = self.tmp / "fixture-target.txt"
        self.artifact_dir = self.tmp / "artifacts"

    def _run_runner(
        self,
        *args: str,
        cargo_target_dir: str | None = None,
    ) -> subprocess.CompletedProcess[str]:
        if self.cargo_log.exists():
            self.cargo_log.unlink()
        if self.fixture_log.exists():
            self.fixture_log.unlink()

        env = os.environ.copy()
        env.pop("CARGO_TARGET_DIR", None)
        env.pop("WAYLAND_DISPLAY", None)
        env["DISPLAY"] = ":99"
        env["PATH"] = f"{self.stub_bin}:{env.get('PATH', '/usr/bin:/bin')}"
        env["CUA_E2E_INTERNAL_LANE"] = "shared"
        env["CUA_E2E_HARNESS_FILTER"] = "electron"
        env["CUA_E2E_ARTIFACT_DIR"] = str(self.artifact_dir)
        env["STUB_CARGO_LOG"] = str(self.cargo_log)
        env["STUB_FIXTURE_LOG"] = str(self.fixture_log)
        if cargo_target_dir is not None:
            env["CARGO_TARGET_DIR"] = cargo_target_dir

        return subprocess.run(
            ["bash", str(self.runner), *args],
            cwd=self.caller_cwd,
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )

    def _cargo_calls(self) -> list[dict[str, str | None]]:
        if not self.cargo_log.exists():
            return []
        return [
            json.loads(line)
            for line in self.cargo_log.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]

    def _stage_electron_fixture(self) -> None:
        _write_executable(
            self.rust_root / "test-apps/harness-electron/CuaTestHarness.Electron",
            "#!/usr/bin/env bash\nexit 0\n",
        )

    def _assert_reached_preflight(
        self,
        result: subprocess.CompletedProcess[str],
        expected_subcmds: list[str],
        expected_env: str | None,
        expected_bin: Path,
    ) -> None:
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, output)
        self.assertIn("[PREFLIGHT]", output)
        self.assertIn("PREFLIGHT_DRIVER_OK", output)
        self.assertIn("Linux E2E environment preflight failed", output)

        calls = self._cargo_calls()
        self.assertEqual([c["subcmd"] for c in calls], expected_subcmds)
        for call in calls:
            self.assertEqual(call["cwd"], str(self.rust_root))
            self.assertEqual(call["cargo_target_dir"], expected_env)
            self.assertEqual(call["cua_test_driver_bin"], str(expected_bin))

    def test_unset_cargo_target_dir_preserves_default_layout(self) -> None:
        result = self._run_runner(cargo_target_dir=None)
        self._assert_reached_preflight(
            result,
            ["build", "test", "run"],
            None,
            self.rust_root / "target/release/cua-driver",
        )
        self.assertEqual(self.fixture_log.read_text(encoding="utf-8").strip(), "unset")

    def test_absolute_cargo_target_dir_is_honored(self) -> None:
        custom_target = self.tmp / "external volume/cargo target"
        result = self._run_runner(cargo_target_dir=f"{custom_target}/")
        self.assertFalse((self.rust_root / "target/release/cua-driver").exists())
        self._assert_reached_preflight(
            result,
            ["build", "test", "run"],
            str(custom_target),
            custom_target / "release/cua-driver",
        )
        self.assertEqual(
            self.fixture_log.read_text(encoding="utf-8").strip(),
            f"set:{custom_target}",
        )

    def test_relative_cargo_target_dir_resolves_against_rust_workspace(self) -> None:
        rel_target = "custom target/nested"
        expected_target = self.rust_root / rel_target
        result = self._run_runner(cargo_target_dir=f"{rel_target}/")
        self.assertFalse((self.rust_root / "target/release/cua-driver").exists())
        self.assertFalse((self.caller_cwd / rel_target).exists())
        self._assert_reached_preflight(
            result,
            ["build", "test", "run"],
            str(expected_target),
            expected_target / "release/cua-driver",
        )
        self.assertEqual(
            self.fixture_log.read_text(encoding="utf-8").strip(),
            f"set:{expected_target}",
        )

    def test_no_build_honors_unset_absolute_and_relative_target_dirs(self) -> None:
        self._stage_electron_fixture()
        abs_target = self.tmp / "prebuilt abs target"
        rel_target = "prebuilt rel target"
        cases = [
            ("unset", None, None, self.rust_root / "target/release/cua-driver"),
            ("absolute", str(abs_target), str(abs_target), abs_target / "release/cua-driver"),
            (
                "relative",
                rel_target,
                str(self.rust_root / rel_target),
                self.rust_root / rel_target / "release/cua-driver",
            ),
        ]
        for label, configured, expected_env, expected_bin in cases:
            with self.subTest(case=label):
                default_bin = self.rust_root / "target/release/cua-driver"
                if default_bin.exists():
                    default_bin.unlink()
                _write_executable(expected_bin, "#!/usr/bin/env bash\nexit 0\n")
                result = self._run_runner("--no-build", cargo_target_dir=configured)
                self.assertFalse(self.fixture_log.exists())
                self._assert_reached_preflight(
                    result,
                    ["test", "run"],
                    expected_env,
                    expected_bin,
                )

    def test_empty_cargo_target_dir_fails_before_build(self) -> None:
        result = self._run_runner(cargo_target_dir="")
        self.assertEqual(result.returncode, 2)
        self.assertIn("CARGO_TARGET_DIR must not be empty", result.stderr)
        self.assertEqual(self._cargo_calls(), [])


if __name__ == "__main__":
    unittest.main()
