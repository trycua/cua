"""Policy: workflows install apt packages through ./.github/actions/apt-install.

Raw `apt-get install` on a hosted runner hangs for the whole job timeout when
the Ubuntu mirror serves packages slowly, with no message saying why. The
apt-install action caches the .debs, caps each attempt at 5 minutes, retries
3 times and prints why it failed. Use it instead of raw apt-get.

Container jobs and `docker run` use .github/scripts/apt-retry.sh (same retries and
5-minute cap). A file may keep raw apt only if it is listed in ALLOWED with the reason. The
count is a ceiling: lowering it is welcome, raising it needs a real reason.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
APT = re.compile(r"\bapt(?:-get)?\b(?: +-\S+)* +install\b|\bapt_retry +apt-get\b")

ALLOWED = {
    ".github/workflows/cd-cua-sdk.yml": (3, "debian:11 container with its own bounded retry and snapshot.debian.org fallback"),
    ".github/workflows/cd-rust-cua-driver.yml": (2, "debian:11 container with its own bounded retry and snapshot.debian.org fallback"),
    ".github/workflows/ci-cua-driver-preflight.yml": (1, "runs inside the sandbox guest (guest-setup), which has no repo checkout"),
    ".github/workflows/ci-cua-sandbox-doctor.yml": (1, "runs inside the sandbox guest (guest-setup), which has no repo checkout"),
    ".github/workflows/ci-cua-spaces-desktop-install-smoke.yml": (1, "installs a local .deb file, not packages from the mirror"),
    ".github/workflows/ci-distro-compat-cua-driver.yml": (1, "container job with no checkout or git yet; the retry loop is inline (same policy as apt-retry.sh)"),
    ".github/workflows/nightly-hyprland-plugin-requalify.yml": (1, "best-effort kernel-version package (|| true), a failure must not fail the job"),
    ".github/workflows/release-rollout.yml": (1, "installed only when xmllint is missing"),
}


def raw_apt_counts():
    counts = {}
    files = list((ROOT / ".github/workflows").glob("*.y*ml"))
    files += (ROOT / ".github/actions").glob("*/action.y*ml")
    for path in files:
        rel = path.relative_to(ROOT).as_posix()
        if rel == ".github/actions/apt-install/action.yml":
            continue
        n = sum(
            1
            for line in path.read_text().splitlines()
            if not line.lstrip().startswith("#") and APT.search(line)
        )
        if n:
            counts[rel] = n
    return counts


class AptPolicy(unittest.TestCase):
    def test_no_new_raw_apt_install(self):
        counts = raw_apt_counts()
        bad = []
        for rel, n in sorted(counts.items()):
            allowed = ALLOWED.get(rel, (0, ""))[0]
            if n > allowed:
                bad.append(f"{rel}: {n} raw apt install line(s), {allowed} allowed")
        self.assertFalse(
            bad,
            "Use ./.github/actions/apt-install (cached, 5 minute attempts, 3 retries, "
            "clear error) instead of raw apt-get install, or add an ALLOWED entry "
            "with the reason:\n" + "\n".join(bad),
        )

    def test_allowlist_has_no_stale_entries(self):
        counts = raw_apt_counts()
        stale = [r for r, (n, _) in ALLOWED.items() if counts.get(r, 0) < n]
        self.assertFalse(stale, f"lower or remove these ALLOWED entries: {stale}")

    def test_local_apt_action_has_a_checkout_before_it(self):
        """A job that uses a local action must check the repo out first."""
        bad = []
        for path in sorted((ROOT / ".github/workflows").glob("*.y*ml")):
            job = None
            checked_out = False
            in_jobs = False
            for line in path.read_text().splitlines():
                if re.match(r"jobs:\s*$", line):
                    in_jobs = True
                    continue
                if in_jobs and re.match(r"  [\w-]+:\s*$", line):
                    job, checked_out = line.strip().rstrip(":"), False
                    continue
                if re.match(r"\s*-?\s*uses:\s*actions/checkout@", line):
                    checked_out = True
                if (
                    job
                    and not checked_out
                    and re.match(r"\s*-?\s*uses:\s*\./\.github/actions/apt-install\b", line)
                ):
                    bad.append(f"{path.name}: job {job}")
        self.assertFalse(
            bad,
            "Add actions/checkout (sparse-checkout: .github/actions) before "
            "the local apt-install action in:\n" + "\n".join(bad),
        )


if __name__ == "__main__":
    unittest.main()
