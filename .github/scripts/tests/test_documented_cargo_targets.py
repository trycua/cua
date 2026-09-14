import json
from pathlib import Path
import shlex
import subprocess

import pytest


ROOT = Path(__file__).resolve().parents[3]


@pytest.mark.parametrize("document", ["TESTING.md", "libs/cua-driver/rust/README.md"])
def test_documented_integration_targets_exist(document):
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
        cwd=ROOT / "libs/cua-driver/rust",
        text=True,
    ))
    targets = {
        package["name"]: {target["name"] for target in package["targets"] if "test" in target["kind"]}
        for package in metadata["packages"]
    }
    commands = [
        shlex.split(line) for line in (ROOT / document).read_text().splitlines()
        if line.startswith("cargo test ") and "--test" in line
    ]
    assert commands, f"no integration test examples found in {document}"
    for command in commands:
        package = command[command.index("-p") + 1]
        requested = {target for flag, target in zip(command, command[1:]) if flag == "--test"}
        assert requested and requested <= targets[package], command
