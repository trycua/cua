"""Reset NA-04: the committee folder with one locked file and one Spotlight comment (in the VM: no Finder windows)."""

import subprocess
import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, finder, mac  # noqa: E402

META = common.read_json(BUNDLE / "reset" / "meta.json")


def files(workspace: Path) -> None:
    folder = workspace / META["folder"]
    for name, text in META["comments"].items():
        finder.set_comment(folder / name, text)
    for name in META["locked"]:
        subprocess.run(["/usr/bin/chflags", "uchg", str(folder / name)], check=True)


def app_state(workspace: Path) -> None:
    mac.finder_close_windows()


if __name__ == "__main__":
    if "--workspace" in sys.argv:  # a locked file from the last trial would stop the reset from deleting it
        finder.unlock_tree(Path(sys.argv[sys.argv.index("--workspace") + 1]))
    common.app_reset_main(BUNDLE, "NA-04", files, app_state)
