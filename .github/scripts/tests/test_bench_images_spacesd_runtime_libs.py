"""The bench image build runs the baked-in cua-spacesd once in a bare ubuntu:22.04
container (`cua-spacesd build-info`). The binary links libxkbcommon, so that
container must install it; without it the step dies with exit 127
("error while loading shared libraries: libxkbcommon.so.0") and every
cd-images-spacesd-release run fails at build/vm.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = (ROOT / "scripts/bench-images/build-qcow2.sh").read_text()


def test_build_info_container_installs_spacesd_runtime_libs():
    line = next(l for l in SCRIPT.splitlines() if "apt-get install" in l and "build-info" in l)
    for pkg in ("libx11-6", "libxi6", "libxkbcommon0"):
        assert f" {pkg} " in line, f"{pkg} missing from the build-info container install"
