"""Guard the pre-checkout bootstrap; native container CI proves installation."""

from pathlib import Path
import re
import shlex
import subprocess
import textwrap
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github/workflows/cd-rust-cua-driver.yml"


class TestDriverLinuxBootstrap(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        workflow = WORKFLOW.read_text()
        # This block must remain self-contained because it runs before checkout.
        block = re.search(
            r"      - name: Install base tooling \(container is bare\)\n"
            r"        run: \|\n((?:          .*\n|\n)+)",
            workflow,
        )
        if block is None:
            raise AssertionError("release bootstrap block not found")
        cls.bootstrap = textwrap.dedent(block[1])

    def test_shell_syntax(self) -> None:
        subprocess.run(["sh", "-n"], input=self.bootstrap, text=True, check=True)

    def test_only_frozen_signed_sources_disable_expiry(self) -> None:
        sources = re.search(
            r"cat > /etc/apt/sources.list <<'EOF'\n(.*?)\nEOF", self.bootstrap, re.S
        )
        self.assertIsNotNone(sources)
        lines = sources[1].splitlines()
        self.assertEqual(len(lines), 4)
        suites = set()
        for line in lines:
            source = re.fullmatch(
                r"deb \[check-valid-until=no "
                r"signed-by=/usr/share/keyrings/debian-archive-keyring.gpg\] "
                r"http://snapshot.debian.org/archive/"
                r"(debian|debian-security|debian-archive)/20260901T000000Z/"
                r"(debian/)? "
                r"(bullseye(?:-updates|-security|-backports)?) main",
                line,
            )
            self.assertIsNotNone(source, line)
            archive, subdirectory, suite = source.groups()
            self.assertEqual(
                (archive, subdirectory),
                {
                    "bullseye": ("debian", None),
                    "bullseye-updates": ("debian", None),
                    "bullseye-security": ("debian-security", None),
                    "bullseye-backports": ("debian-archive", "debian/"),
                }[suite],
            )
            suites.add(suite)
        self.assertEqual(
            suites,
            {"bullseye", "bullseye-updates", "bullseye-security", "bullseye-backports"},
        )
        remaining = self.bootstrap.replace(sources[1], "")
        # The documented archive.debian.org fallback (past Valid-Until by
        # design) is the only other place expiry checks may be disabled.
        fallback = re.search(
            r"cat > /etc/apt/sources.list <<'EOF'\n(deb \[check-valid-until=no\] "
            r"http://archive.debian.org/.*?)\nEOF",
            remaining,
            re.S,
        )
        self.assertIsNotNone(fallback)
        self.assertEqual(len(fallback[1].splitlines()), 4)
        self.assertIn("http://archive.debian.org/debian bullseye-backports main", fallback[1])
        remaining = remaining.replace(fallback[1], "")
        self.assertNotRegex(remaining.lower(), r"check-valid-until|check-date")
        self.assertNotRegex(
            self.bootstrap.lower(),
            r"trusted\s*=|allow-unauthenticated|allowinsecure|allow-insecure|"
            r"allowweak|allow-weak|verify-peer|verify-host",
        )

    def test_apt_retries_snapshot_first_then_archive_fallback(self) -> None:
        self.assertIn('Acquire::Retries "5";', self.bootstrap)
        self.assertIn('Acquire::http::Timeout "30";', self.bootstrap)
        self.assertIn("apt mirror used: snapshot.debian.org 20260901T000000Z", self.bootstrap)
        self.assertIn("apt mirror used: archive.debian.org", self.bootstrap)
        # Snapshot sources are written first, the archive fallback second.
        self.assertLess(
            self.bootstrap.index("snapshot.debian.org/archive/debian/"),
            self.bootstrap.index("http://archive.debian.org/debian bullseye main"),
        )
        # Image-provided extra sources stay ignored in both attempts.
        self.assertIn("Dir::Etc::sourceparts=-", self.bootstrap)
        pkgs = re.search(r'^PACKAGES="([^"]*)"', self.bootstrap, re.M)
        self.assertIsNotNone(pkgs)
        self.assertEqual(
            set(pkgs[1].split()),
            {
                "git", "ca-certificates", "curl", "python3", "build-essential", "pkg-config",
                "libx11-dev", "libxi-dev", "libxtst-dev", "libxext-dev", "libwayland-dev",
                "libxkbcommon-dev", "libclang-11-dev",
            },
        )
        # PipeWire headers must come from backports explicitly: bullseye's own
        # 0.3.19 headers do not compile libspa-sys, and backports has a lower
        # APT priority than the release suite.
        backports = re.search(r'^BACKPORTS_PACKAGES="([^"]*)"', self.bootstrap, re.M)
        self.assertIsNotNone(backports)
        self.assertEqual(set(backports[1].split()), {"libpipewire-0.3-dev", "libspa-0.2-dev"})
        self.assertIn(
            "install -y --no-install-recommends -t bullseye-backports $BACKPORTS_PACKAGES",
            self.bootstrap,
        )


if __name__ == "__main__":
    unittest.main()
