"""Static wiring guards for NixOS Linux desktop coverage."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestNixOsDesktopMatrix(unittest.TestCase):
    """Keep desktop checks, automatic smoke, and evidence uploads wired."""

    def test_flake_exports_x11_and_all_supported_wayland_sessions(self) -> None:
        flake = (REPO_ROOT / "flake.nix").read_text()
        for desktop in ("xfce-labwc", "xfce-sway", "kde", "gnome"):
            self.assertIn(f'"{desktop}"', flake)
        for check in (
            "cua-driver-integration",
            "cua-driver-screenshot",
            "cua-driver-linux-cursor-click-gif",
            "cua-driver-linux-background-terminal-gif",
            "cua-driver-linux-parallel-drag-xserver",
            "cua-driver-wayland-${desktop}-${scenario}",
            "cua-driver-wayland-${desktop}-background-gui-${app}",
        ):
            self.assertIn(check, flake)

    def test_workflow_keeps_automatic_smoke_and_dispatch_full_matrix(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/e2e-nixos-desktop.yml").read_text()
        self.assertIn("pull_request:", workflow)
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("x11-integration-capture", workflow)
        self.assertIn("sway-native-input", workflow)
        self.assertIn("desktop: [x11, xfce-labwc, xfce-sway, kde, gnome]", workflow)
        self.assertIn("if: always()", workflow)
        self.assertIn("auto-allocate-uids = true", workflow)
        self.assertIn("extra-system-features = uid-range", workflow)
        self.assertIn("actions/upload-artifact@65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08", workflow)
        self.assertIn("GITHUB_STEP_SUMMARY", workflow)

    def test_visual_test_sources_remain_present(self) -> None:
        required = (
            "nix/cua-driver/tests/integration.nix",
            "nix/cua-driver/tests/screenshot.nix",
            "nix/cua-driver/tests/linux-cursor-click-gif.nix",
            "nix/cua-driver/tests/linux-background-terminal-gif.nix",
            "nix/cua-driver/tests/linux-parallel-drag-xserver.nix",
            "nix/cua-driver/tests/linux-background-gui.nix",
            "nix/cua-driver/tests/wayland/session.nix",
            "nix/cua-driver/tests/wayland/integration.nix",
            "nix/cua-driver/tests/wayland/screenshot.nix",
            "nix/cua-driver/tests/wayland/cursor-click-gif.nix",
            "nix/cua-driver/tests/wayland/background-terminal-gif.nix",
            "nix/cua-driver/tests/wayland/parallel-drag.nix",
            "nix/cua-driver/tests/wayland/background-gui.nix",
        )
        for relative in required:
            self.assertTrue((REPO_ROOT / relative).is_file(), relative)


if __name__ == "__main__":
    unittest.main()
