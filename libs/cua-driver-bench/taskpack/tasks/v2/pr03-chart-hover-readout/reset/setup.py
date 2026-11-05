"""Reset PR-03: copy the fixture into the workspace."""

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common  # noqa: E402


def setup(workspace: Path) -> None:
    common.seed_chrome_profile(workspace)


if __name__ == "__main__":
    common.reset_main(BUNDLE, "PR-03", setup)
