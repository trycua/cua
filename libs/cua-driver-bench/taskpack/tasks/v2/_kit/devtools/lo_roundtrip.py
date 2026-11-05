#!/usr/bin/env python3
"""Development-only: open a document in a headless LibreOffice and save it again (never in trials).

    python3 lo_roundtrip.py IN_FILE OUT_DIR [--to fods|ods|fodt|odt|fodp|odp|csv]

It uses a throwaway user profile, so it touches no LibreOffice settings, and runs headless, so no
window opens. Checker tests use it to confirm that a solved document still passes after LibreOffice
re-serialises it (attribute order, repeated cells, styles it rewrites), as it does when an agent saves.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import tempfile
from pathlib import Path

SOFFICE = "/Applications/LibreOffice.app/Contents/MacOS/soffice"


def available() -> bool:
    return Path(SOFFICE).exists()


def roundtrip(src: Path, out_dir: Path, to: str | None = None) -> Path:
    to = to or src.suffix.lstrip(".")
    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="lo-profile-") as prof, tempfile.TemporaryDirectory(prefix="lo-out-") as tmp_out:
        subprocess.run(
            [SOFFICE, "--headless", "--norestore", "--nolockcheck", f"-env:UserInstallation={Path(prof).as_uri()}",
             "--convert-to", to, "--outdir", tmp_out, str(src)],
            check=True, capture_output=True, timeout=180,
        )
        made = Path(tmp_out) / (src.stem + "." + to.split(":")[0])
        if not made.exists():
            raise RuntimeError(f"LibreOffice wrote nothing for {src.name}")
        target = out_dir / made.name
        shutil.copyfile(made, target)
        return target


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("src", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--to")
    a = ap.parse_args()
    print(roundtrip(a.src, a.out_dir, a.to))
