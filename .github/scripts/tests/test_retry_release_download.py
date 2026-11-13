"""retry.sh: a retried `gh release download` overwrites the files a failed attempt left."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

RETRY = Path(__file__).resolve().parents[1] / "retry.sh"

# A fake `gh` that records its arguments, fails the first call (like a GitHub 500
# after some assets were written) and then succeeds only with --clobber.
FAKE_GH = """#!/usr/bin/env bash
echo "$*" >> "$LOG"
n=$(wc -l < "$LOG")
if [ "$n" -eq 1 ]; then exit 1; fi
case " $* " in *" --clobber "*|*" --skip-existing "*|*" --output "*) exit 0 ;; esac
exit 1
"""


def run(args):
    with tempfile.TemporaryDirectory() as tmp:
        gh = Path(tmp) / "gh"
        gh.write_text(FAKE_GH)
        gh.chmod(0o755)
        log = Path(tmp) / "log"
        env = dict(os.environ, PATH=f"{tmp}:{os.environ['PATH']}", LOG=str(log))
        script = f'source "{RETRY}"; sleep() {{ :; }}; retry {args}'
        proc = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True)
        return proc.returncode, log.read_text().splitlines()


class RetryReleaseDownload(unittest.TestCase):
    def test_retried_download_gets_clobber(self):
        code, calls = run('gh release download "v1" --dir assets')
        self.assertEqual(code, 0)
        self.assertTrue(all("--clobber" in c for c in calls))

    def test_caller_choice_is_kept(self):
        code, calls = run('gh release download "v1" --dir assets --skip-existing')
        self.assertEqual(code, 0)
        self.assertTrue(all("--clobber" not in c for c in calls))

    def test_other_commands_are_untouched(self):
        code, calls = run("gh api repos/x/y")
        self.assertNotEqual(code, 0)
        self.assertTrue(all("--clobber" not in c for c in calls))


if __name__ == "__main__":
    unittest.main()
