"""Every script that runs the Rust tests raises the open-files limit first.

macOS shells default to 256 open files. The cua-spaces test binaries (host_spaces,
scale, relay_publish, the cua-compat chaos suite) need more and fail with "Too
many open files" on such a Mac, for no code reason (seen on a CI Mac, 10 Nov 2026). So:

  * every tracked shell script that runs `cargo test` / `cargo nextest` carries the
    canonical raise line (marker `# nofile-limit`) before it runs anything;
  * the line works: 256 becomes 10240, and a higher limit is never lowered;
  * bin/ci and the cua-sdk runner raise the limit themselves.
"""

from __future__ import annotations

import re
import resource
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
MARKER = "# nofile-limit"
CARGO_TEST = re.compile(r"(^|[;&|(]\s*|\s)cargo\s+(test|nextest)\b")
# Scripts that only mention cargo test in text, or that run it where the limit is not ours to set.
EXEMPT: set[str] = set()


def tracked_shell_scripts() -> list[Path]:
    out = subprocess.run(["git", "ls-files", "*.sh"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    return [ROOT / line for line in out.splitlines() if line]


def runs_cargo_test(path: Path) -> bool:
    for line in path.read_text(errors="replace").splitlines():
        if line.lstrip().startswith("#"):
            continue
        if CARGO_TEST.search(line):
            return True
    return False


def canonical_line() -> str:
    """The raise line as the scripts carry it (the one in scripts/ci/macos/run-rust-e2e.sh)."""
    for line in (ROOT / "scripts/ci/macos/run-rust-e2e.sh").read_text().splitlines():
        if line.endswith(MARKER):
            return line
    raise AssertionError("scripts/ci/macos/run-rust-e2e.sh lacks the nofile-limit line")


def soft_after(line: str, start: int) -> str:
    """The soft limit a shell reports after running `line` with `start` as its soft limit."""
    script = f"ulimit -Sn {start}; {line}\nulimit -Sn"
    return subprocess.run(["bash", "-c", script], capture_output=True, text=True, check=True).stdout.strip()


class ScriptsRaiseTheLimit(unittest.TestCase):
    def test_every_cargo_test_script_has_the_line(self):
        line = canonical_line()
        missing = []
        for path in tracked_shell_scripts():
            rel = path.relative_to(ROOT).as_posix()
            if rel in EXEMPT or not runs_cargo_test(path):
                continue
            if line not in path.read_text(errors="replace").splitlines():
                missing.append(rel)
        self.assertEqual(
            missing,
            [],
            "these scripts run cargo test without raising the open-files limit; add this line after `set -e...`:\n"
            f"{canonical_line()}",
        )

    def test_the_line_comes_before_the_first_cargo_call(self):
        for path in tracked_shell_scripts():
            if not runs_cargo_test(path):
                continue
            lines = path.read_text(errors="replace").splitlines()
            first_cargo = next(
                i for i, l in enumerate(lines) if not l.lstrip().startswith("#") and CARGO_TEST.search(l)
            )
            marker = next((i for i, l in enumerate(lines) if l.endswith(MARKER)), None)
            self.assertIsNotNone(marker, path)
            self.assertLess(marker, first_cargo, f"{path.relative_to(ROOT)}: raise the limit before cargo test")

    def test_the_scanner_sees_a_real_invocation_and_ignores_comments(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            real = Path(d, "a.sh")
            real.write_text("#!/bin/sh\nset -e\n  cargo test -p x\n")
            talk = Path(d, "b.sh")
            talk.write_text("#!/bin/sh\n# run cargo test later\necho cargo testing\n")
            self.assertTrue(runs_cargo_test(real))
            self.assertFalse(runs_cargo_test(talk))


@unittest.skipIf(sys.platform == "win32", "ulimit is POSIX")
class TheLineWorks(unittest.TestCase):
    def setUp(self):
        _, self.hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        if self.hard not in (resource.RLIM_INFINITY,) and self.hard < 10240:
            self.skipTest("hard open-files limit below 10240")

    def test_default_256_is_raised(self):
        self.assertGreaterEqual(int(soft_after(canonical_line(), 256)), 10240)

    def test_a_higher_limit_is_kept(self):
        want = 20000 if self.hard == resource.RLIM_INFINITY else min(20000, self.hard)
        if want <= 10240:
            self.skipTest("hard limit leaves no room above 10240")
        self.assertEqual(int(soft_after(canonical_line(), want)), want)

    def test_does_not_abort_under_set_e(self):
        script = f"set -eu\n{canonical_line()}\necho survived"
        out = subprocess.run(["bash", "-c", "ulimit -Sn 256; " + script], capture_output=True, text=True)
        self.assertEqual(out.stdout.strip(), "survived", out.stderr)


class PythonRunnersRaiseTheLimit(unittest.TestCase):
    def run_in_256(self, code: str) -> int:
        script = f"import resource\nsoft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)\nresource.setrlimit(resource.RLIMIT_NOFILE, (256, hard))\n{code}\nprint(resource.getrlimit(resource.RLIMIT_NOFILE)[0])"
        out = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, check=True)
        return int(out.stdout.strip())

    @unittest.skipIf(sys.platform == "win32", "RLIMIT_NOFILE is POSIX")
    def test_bin_ci_raises_256(self):
        text = (ROOT / "bin/ci").read_text()
        start = text.index("def raise_open_file_limit")
        end = text.index("def main(argv")
        self.assertRegex(text, r"(?m)^    raise_open_file_limit\(\)$")
        code = text[start:end] + "\nraise_open_file_limit()"
        self.assertGreaterEqual(self.run_in_256(code), 10240)

    @unittest.skipIf(sys.platform == "win32", "RLIMIT_NOFILE is POSIX")
    def test_cua_sdk_runner_raises_256(self):
        text = (ROOT / "tests/e2e/cua-sdk/run.py").read_text()
        start = text.index("def raise_open_file_limit")
        end = text.index("def main() -> int:")
        self.assertIn("raise_open_file_limit()\n    ap = argparse.ArgumentParser", text)
        self.assertGreaterEqual(self.run_in_256(text[start:end] + "\nraise_open_file_limit()"), 10240)


if __name__ == "__main__":
    unittest.main()
