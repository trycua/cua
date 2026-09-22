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
                r"http://(?:snapshot.debian.org/archive/(debian|debian-security)/"
                r"20260901T000000Z|archive.debian.org/(debian))/ "
                r"(bullseye(?:-updates|-security|-backports)?) main",
                line,
            )
            self.assertIsNotNone(source, line)
            snapshot_archive, retired_archive, suite = source.groups()
            if suite == "bullseye-backports":
                # Retired from the snapshot mirror; only the immutable archive serves it.
                self.assertEqual(retired_archive, "debian")
            else:
                self.assertEqual(
                    snapshot_archive,
                    "debian-security" if suite == "bullseye-security" else "debian",
                )
            suites.add(suite)
        self.assertEqual(
            suites,
            {"bullseye", "bullseye-updates", "bullseye-security", "bullseye-backports"},
        )
        remaining = self.bootstrap.replace(sources[1], "")
        self.assertNotRegex(remaining.lower(), r"check-valid-until|check-date")
        self.assertNotRegex(
            self.bootstrap.lower(),
            r"trusted\s*=|allow-unauthenticated|allowinsecure|allow-insecure|"
            r"allowweak|allow-weak|verify-peer|verify-host",
        )

    def test_apt_uses_only_snapshot_sources_and_full_dependencies(self) -> None:
        commands = [
            shlex.split(line)
            for line in self.bootstrap.replace("\\\n", " ").splitlines()
            if line.startswith("apt-get ")
        ]
        self.assertEqual(commands[0], ["apt-get", "-o", "Dir::Etc::sourceparts=-", "update"])
        self.assertEqual(
            commands[1][:6],
            ["apt-get", "-o", "Dir::Etc::sourceparts=-", "install", "-y", "--no-install-recommends"],
        )
        self.assertEqual(len(commands), 3)
        self.assertEqual(
            set(commands[1][6:]),
            {
                "git", "ca-certificates", "curl", "python3", "build-essential", "pkg-config",
                "libx11-dev", "libxi-dev", "libxtst-dev", "libxext-dev", "libwayland-dev",
                "libxkbcommon-dev", "libclang-11-dev",
            },
        )
        # PipeWire headers must come from backports explicitly: bullseye's own
        # 0.3.19 headers do not compile libspa-sys, and backports has a lower
        # APT priority than the release suite.
        self.assertEqual(
            commands[2][:8],
            [
                "apt-get", "-o", "Dir::Etc::sourceparts=-", "install", "-y",
                "--no-install-recommends", "-t", "bullseye-backports",
            ],
        )
        self.assertEqual(set(commands[2][8:]), {"libpipewire-0.3-dev", "libspa-0.2-dev"})
        self.assertLess(self.bootstrap.index("\nEOF\n"), self.bootstrap.index("apt-get "))


if __name__ == "__main__":
    unittest.main()
