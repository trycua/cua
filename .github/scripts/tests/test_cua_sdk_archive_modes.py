"""The per-platform archives the SDK release attaches keep their programs executable.

`actions/download-artifact` drops file modes, so the publish job has to set the
executable bit again before it archives each `native-*` directory. A `cua`
extracted from `cua-<version>-<platform>.tar.gz` that is mode 0644 fails with
"Permission denied" (issue 5709).
"""

from __future__ import annotations

import subprocess
import tarfile
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/cd-cua-sdk.yml"


def archive_loop() -> str:
    """The loop of "Attach artifacts to the release" that writes the native archives."""
    steps = yaml.safe_load(WORKFLOW.read_text())["jobs"]["publish"]["steps"]
    run = next(s["run"] for s in steps if s.get("name") == "Attach artifacts to the release")
    lines = run[run.index("for d in dist/native-*; do") :].splitlines()
    loop = lines[: lines.index("done") + 1]
    return "\n".join(loop).replace("${{ needs.version.outputs.version }}", "9.9.9")


def test_native_archives_ship_executable_programs(tmp_path: Path) -> None:
    native = tmp_path / "dist" / "native-linux-arm64-gnu"
    native.mkdir(parents=True)
    for name in (
        "cua",
        "cua-spacesd-linux-aarch64",
        "cua-microvm-init",
        "libcua_sdk.so",
        "cua_node_runtime.node",
    ):
        path = native / name
        path.write_bytes(b"x")
        path.chmod(0o644)  # what download-artifact leaves behind
    (tmp_path / "release").mkdir()

    subprocess.run(["bash", "-eu", "-c", archive_loop()], cwd=tmp_path, check=True)

    with tarfile.open(tmp_path / "release" / "cua-9.9.9-linux-arm64-gnu.tar.gz") as tar:
        modes = {Path(m.name).name: m.mode & 0o777 for m in tar.getmembers() if m.isfile()}
    for program in ("cua", "cua-spacesd-linux-aarch64", "cua-microvm-init"):
        assert modes[program] == 0o755, (program, oct(modes[program]))
    # Libraries are not programs.
    assert modes["libcua_sdk.so"] == 0o644
    assert modes["cua_node_runtime.node"] == 0o644
