"""Policy: hosted Linux runners get /dev/kvm through scripts/images/enable-kvm.sh.

Without KVM, QEMU falls back to TCG software emulation, which boots a desktop
image in 900-1300+ s on a hosted runner (the osworld containerdisk verify
timed out that way). Workflows must enable KVM with the udev-rule script, not
a one-off `chmod 666 /dev/kvm` that a later QEMU install can undo, and the
bench verify must require KVM on amd64 so a regression is an error, not a slow
timeout.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/images/enable-kvm.sh"
CHMOD = re.compile(r"chmod\s+(?:-\w+\s+)?(?:666|a\+rw|o\+rw)\s+/dev/kvm")
# The sandbox action is consumed by other repos, so it cannot call the script.
ALLOWED_CHMOD = {".github/actions/cua-sandbox/cua-sandbox.sh", "scripts/images/enable-kvm.sh"}


class EnableKvmPolicy(unittest.TestCase):
    def test_script_uses_the_documented_udev_rule_and_can_require(self):
        text = SCRIPT.read_text()
        self.assertIn('KERNEL=="kvm", GROUP="kvm", MODE="0666", OPTIONS+="static_node=kvm"', text)
        self.assertIn("99-kvm4all.rules", text)
        self.assertIn("udevadm control --reload-rules", text)
        self.assertIn("udevadm trigger --name-match=kvm", text)
        self.assertIn("--require", text)

    def test_no_raw_chmod_of_dev_kvm(self):
        offenders = []
        files = list((ROOT / ".github/workflows").glob("*.yml")) + list((ROOT / "scripts").rglob("*.sh"))
        files += list((ROOT / ".github/actions").rglob("*"))
        for f in files:
            if not f.is_file():
                continue
            rel = f.relative_to(ROOT).as_posix()
            if rel in ALLOWED_CHMOD:
                continue
            for n, line in enumerate(f.read_text(errors="ignore").splitlines(), 1):
                if CHMOD.search(line):
                    offenders.append(f"{rel}:{n}: {line.strip()}")
        self.assertEqual(offenders, [], "use scripts/images/enable-kvm.sh instead of chmod /dev/kvm")

    def test_bench_verify_requires_kvm_on_amd64(self):
        text = (ROOT / ".github/workflows/cd-bench-images.yml").read_text()
        self.assertIn("CUA_REQUIRE_KVM: ${{ matrix.arch == 'amd64' && '1' || '' }}", text)
        self.assertIn("scripts/images/enable-kvm.sh --require", text)


if __name__ == "__main__":
    unittest.main()
