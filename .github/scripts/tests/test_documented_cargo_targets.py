import json
from pathlib import Path
import shlex
import subprocess
import unittest


ROOT = Path(__file__).resolve().parents[3]


class DocumentedCargoTargetsTest(unittest.TestCase):
    def test_documented_integration_targets_exist(self):
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
            cwd=ROOT / "libs/cua-driver/rust",
            text=True,
        ))
        targets = {
            package["name"]: {target["name"] for target in package["targets"] if "test" in target["kind"]}
            for package in metadata["packages"]
        }
        for document in ("TESTING.md", "libs/cua-driver/rust/README.md"):
            commands = [
                shlex.split(line)
                for line in (ROOT / document).read_text().splitlines()
                if line.startswith("cargo test ") and "--test" in line
            ]
            self.assertTrue(commands, f"no integration test examples found in {document}")
            for command in commands:
                package = command[command.index("-p") + 1]
                for flag, target in zip(command, command[1:]):
                    if flag == "--test":
                        with self.subTest(document=document, target=target):
                            self.assertIn(target, targets[package])
