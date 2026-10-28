#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Builds every size of the Cua Spaces app icon from the two chosen masters.

  src/app-icon-1024.png        the icon, full bleed and opaque: the terracotta koala
                               keycap on its background (no corners, no margin)
  src/app-icon-small-1024.png  the same icon with a larger key and no stars, for
                               16 to 64 px, where the full art turns to mush
  ../../Support/AppIcon.icon   the macOS 26 Icon Composer source (layers; not built here)

The masters are square; this script shapes them per platform:
  macOS iconset  Apple squircle, 824 px body on a 1024 canvas, soft drop shadow
                 (macOS 15 and older; macOS 26 draws Support/AppIcon.icon instead)
  tauri set      square with rounded corners (22%), 3% margin, for Windows/Linux

  app-icon.py iconset DIR            icon_16x16.png ... icon_512x512@2x.png, for iconutil
  app-icon.py master FILE            icon-1024.png of the rounded-square plate
  app-icon.py tauri DIR              the Tauri / Electron / Linux / Windows set
                                     (png sizes, icon.png, icon.ico), no icon.icns

Sizes up to 32 px come from the small master, sharpened after downsampling;
33 to 64 px from the small master as is; larger ones from the full master.
"""
import math
import pathlib
import sys

from PIL import Image, ImageDraw, ImageFilter

HERE = pathlib.Path(__file__).resolve().parent
FULL = Image.open(HERE / "src/app-icon-1024.png").convert("RGBA")
SMALL = Image.open(HERE / "src/app-icon-small-1024.png").convert("RGBA")


S = 1024


def squircle_mask(body: int, n: float = 5.0, ss: int = 4) -> Image.Image:
    """Apple-like continuous corners: a superellipse, body px wide, centred on the canvas."""
    pts = []
    for i in range(720):
        t = 2 * math.pi * i / 720
        c, s = math.cos(t), math.sin(t)
        x = math.copysign(abs(c) ** (2 / n), c)
        y = math.copysign(abs(s) ** (2 / n), s)
        pts.append(((S / 2 + x * body / 2) * ss, (S / 2 + y * body / 2) * ss))
    m = Image.new("L", (S * ss, S * ss), 0)
    ImageDraw.Draw(m).polygon(pts, fill=255)
    return m.resize((S, S), Image.LANCZOS)


def rrect_mask(body: int, radius: float, ss: int = 4) -> Image.Image:
    m = Image.new("L", (S * ss, S * ss), 0)
    o = (S - body) / 2 * ss
    ImageDraw.Draw(m).rounded_rectangle((o, o, S * ss - o - 1, S * ss - o - 1), radius * ss, fill=255)
    return m.resize((S, S), Image.LANCZOS)


def shaped(master: Image.Image, kind: str) -> Image.Image:
    if kind == "squircle":
        body, mask = 824, squircle_mask(824)
    else:
        body, mask = round(S * 0.94), rrect_mask(round(S * 0.94), round(S * 0.94) * 0.22)
    art = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    art.paste(master.resize((body, body), Image.LANCZOS), ((S - body) // 2,) * 2)
    art.putalpha(mask)
    if kind != "squircle":
        return art
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    shadow.putalpha(mask.point(lambda v: int(v * 0.30)))
    shadow = shadow.filter(ImageFilter.GaussianBlur(14))
    out = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    out.alpha_composite(shadow, (0, 10))
    out.alpha_composite(art)
    return out


SHAPES = {"squircle": (shaped(FULL, "squircle"), shaped(SMALL, "squircle")),
          "plate": (shaped(FULL, "plate"), shaped(SMALL, "plate"))}


def pick(size: int, kind: str = "plate") -> Image.Image:
    full, small = SHAPES[kind]
    if size <= 32:
        return small.resize((size, size), Image.LANCZOS).filter(ImageFilter.UnsharpMask(1, 60, 0))
    if size <= 64:
        return small.resize((size, size), Image.LANCZOS)
    return full.resize((size, size), Image.LANCZOS)


def iconset(out: pathlib.Path) -> None:
    for s in (16, 32, 128, 256, 512):
        pick(s, "squircle").save(out / f"icon_{s}x{s}.png")
        pick(s * 2, "squircle").save(out / f"icon_{s}x{s}@2x.png")


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


def master(out: pathlib.Path) -> None:
    SHAPES["plate"][0].save(out)


if __name__ == "__main__" and len(sys.argv) == 3 and sys.argv[1] == "master":
    master(pathlib.Path(sys.argv[2]))
elif __name__ == "__main__" and len(sys.argv) == 3 and sys.argv[1] in ("iconset", "tauri"):
    dest = pathlib.Path(sys.argv[2])
    dest.mkdir(parents=True, exist_ok=True)
    {"iconset": iconset, "tauri": tauri}[sys.argv[1]](dest)
else:
    sys.exit(__doc__)
