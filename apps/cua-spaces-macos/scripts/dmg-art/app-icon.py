#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Builds every size of the Cua Spaces app icon from the two chosen masters.

  src/app-icon-1024.png        the icon (the terracotta koala keycap)
  src/app-icon-small-1024.png  the same icon redrawn with a larger print, for
                               16 to 64 px, where the full art turns to mush

  app-icon.py iconset DIR            icon_16x16.png ... icon_512x512@2x.png, for iconutil
  app-icon.py tauri DIR              the Tauri / Electron / Linux / Windows set
                                     (png sizes, icon.png, icon.ico), no icon.icns

Sizes up to 32 px come from the small master, sharpened after downsampling;
33 to 64 px from the small master as is; larger ones from the full master.
"""
import pathlib
import sys

from PIL import Image, ImageFilter

HERE = pathlib.Path(__file__).resolve().parent
FULL = Image.open(HERE / "src/app-icon-1024.png").convert("RGBA")
SMALL = Image.open(HERE / "src/app-icon-small-1024.png").convert("RGBA")


def pick(size: int) -> Image.Image:
    if size <= 32:
        return SMALL.resize((size, size), Image.LANCZOS).filter(ImageFilter.UnsharpMask(1, 60, 0))
    if size <= 64:
        return SMALL.resize((size, size), Image.LANCZOS)
    return FULL.resize((size, size), Image.LANCZOS)


def iconset(out: pathlib.Path) -> None:
    for s in (16, 32, 128, 256, 512):
        pick(s).save(out / f"icon_{s}x{s}.png")
        pick(s * 2).save(out / f"icon_{s}x{s}@2x.png")


def tauri(out: pathlib.Path) -> None:
    names = {
        "32x32.png": 32, "64x64.png": 64, "128x128.png": 128, "128x128@2x.png": 256, "icon.png": 512,
        "StoreLogo.png": 50, "Square30x30Logo.png": 30, "Square44x44Logo.png": 44, "Square71x71Logo.png": 71,
        "Square89x89Logo.png": 89, "Square107x107Logo.png": 107, "Square142x142Logo.png": 142,
        "Square150x150Logo.png": 150, "Square284x284Logo.png": 284, "Square310x310Logo.png": 310,
    }
    for name, s in names.items():
        pick(s).save(out / name)
    frames = [pick(s) for s in (16, 24, 32, 48, 64, 256)]
    frames[-1].save(out / "icon.ico", format="ICO", sizes=[f.size for f in frames], append_images=frames[:-1])


if __name__ == "__main__" and len(sys.argv) == 3 and sys.argv[1] in ("iconset", "tauri"):
    dest = pathlib.Path(sys.argv[2])
    dest.mkdir(parents=True, exist_ok=True)
    {"iconset": iconset, "tauri": tauri}[sys.argv[1]](dest)
else:
    sys.exit(__doc__)
