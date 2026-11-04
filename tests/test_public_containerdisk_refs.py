import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PRIVATE_REGISTRY = "296062593712.dkr.ecr.us-west-2.amazonaws.com"
PRIVATE_REPOSITORIES = (
    f"{PRIVATE_REGISTRY}/desktop-workspace-duo",
    f"{PRIVATE_REGISTRY}/cua-server-windows",
)


class PublicContainerDiskReferenceTests(unittest.TestCase):
    def test_tracked_files_do_not_reference_private_containerdisk_repositories(self) -> None:
        tracked_files = (
            subprocess.run(
                ["git", "ls-files", "-z"],
                cwd=ROOT,
                check=True,
                capture_output=True,
            )
            .stdout.decode()
            .split("\0")
        )
        violations = []
        for relative_path in tracked_files:
            if not relative_path:
                continue
            path = ROOT / relative_path
            if not path.is_file():
                continue
            try:
                text = path.read_text()
            except UnicodeDecodeError:
                continue
            for repository in PRIVATE_REPOSITORIES:
                if repository in text:
                    violations.append(f"{relative_path}: {repository}")
        self.assertEqual(
            violations,
            [],
            "Private containerDisk references remain:\n" + "\n".join(violations),
        )


if __name__ == "__main__":
    unittest.main()
