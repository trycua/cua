"""Contract tests for the representative Linux desktop wrapper."""

import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
RUNNER = REPO_ROOT / "scripts/ci/linux/run-rust-e2e-desktop.sh"


class TestLinuxE2eDesktop(unittest.TestCase):
    def make_executable(self, path: Path, body: str) -> None:
        path.write_text(textwrap.dedent(body))
        path.chmod(0o755)

    def hyprland_environment(self, directory: Path) -> dict[str, str]:
        hyprctl = directory / "hyprctl"
        self.make_executable(
            hyprctl,
            """\
            #!/usr/bin/env bash
            set -euo pipefail
            printf '%s\\n' "$*" >> "${HYPRCTL_CALLS}"
            if [[ "$*" == "-j monitors" ]]; then
              printf '[{"name":"HDMI-A-1","focused":true}]\\n'
            else
              printf '{}\\n'
            fi
            """,
        )
        delegated = directory / "delegated.sh"
        self.make_executable(
            delegated,
            """\
            #!/usr/bin/env bash
            set -euo pipefail
            printf 'session=%s\\n' "${CUA_E2E_WAYLAND_SESSION:-}"
            printf 'compositor=%s\\n' "${CUA_E2E_COMPOSITOR:-}"
            printf 'inputs=%s\\n' "${CUA_E2E_INPUT_BACKENDS:-}"
            printf 'wayland=%s\\n' "${CUA_DRIVER_RS_ENABLE_WAYLAND:-}"
            printf 'recording_output=%s\\n' "${CUA_WAYLAND_RECORDING_OUTPUT:-}"
            printf 'args=%s\\n' "$*"
            """,
        )
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{directory}:{env['PATH']}",
                "XDG_SESSION_TYPE": "wayland",
                "XDG_CURRENT_DESKTOP": "Hyprland",
                "HYPRLAND_INSTANCE_SIGNATURE": "test-instance",
                "CUA_E2E_HARNESS_FILTER": "electron",
                "CUA_E2E_DESKTOP_RUNNER": str(delegated),
                "HYPRCTL_CALLS": str(directory / "hyprctl-calls.txt"),
            }
        )
        return env

    def test_hyprland_mode_preflights_ipc_and_delegates_exact_environment(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            env = self.hyprland_environment(directory)
            result = subprocess.run(
                [RUNNER, "hyprland", "--no-build"],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                result.stdout.splitlines(),
                [
                    "session=hyprland",
                    "compositor=hyprland",
                    "inputs=atspi,wlr-virtual-pointer",
                    "wayland=1",
                    "recording_output=HDMI-A-1",
                    "args=--no-build",
                ],
            )
            self.assertEqual(
                (directory / "hyprctl-calls.txt").read_text().splitlines(),
                ["-j version", "-j clients", "-j monitors"],
            )

    def test_hyprland_mode_refuses_a_non_hyprland_session(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            env = self.hyprland_environment(directory)
            env["XDG_CURRENT_DESKTOP"] = "GNOME"
            result = subprocess.run(
                [RUNNER, "hyprland"],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(result.returncode, 2)
            self.assertIn("does not identify Hyprland", result.stderr)
            self.assertFalse((directory / "hyprctl-calls.txt").exists())

    def test_usage_advertises_hyprland(self) -> None:
        result = subprocess.run(
            [RUNNER, "unknown"],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            check=False,
        )

        self.assertEqual(result.returncode, 2)
        self.assertIn("{gnome|kde|hyprland|xorg}", result.stderr)


if __name__ == "__main__":
    unittest.main()
