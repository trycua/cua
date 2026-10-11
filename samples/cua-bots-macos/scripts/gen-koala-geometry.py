#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Regenerate Sources/CuaBotsCore/Avatar/KoalaGeometry.swift.

Usage: scripts/gen-koala-geometry.py <koala design source dir>

Reads the koala design source, a folder with `build.mjs` and
`assets/cua-logo-white.svg`: the head blob, head radius table and ear curve
from `build.mjs`, and the nose from the Cua mark in the SVG. The nose and the
eye anchors
are placed for a front-facing head (yaw 0, no roll), the way `runtime.js`
places them. Units are head radii, origin at the head's centroid, y down.
"""
import math
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
src = (root / "build.mjs").read_text()
HEAD = re.search(r'const HEAD = "([^"]+)"', src).group(1)
EAR = re.search(r'const EAR_R = "([^"]+)"', src).group(1)
HEAD_R = [float(v) for v in re.search(r"const HEAD_R = \[([^\]]+)\]", src).group(1).split(",")]
EAR_AT = [float(v) for v in re.search(r"const EAR_AT = \[([^\]]+)\]", src).group(1).split(",")]
svg = (root / "assets" / "cua-logo-white.svg").read_text()
nose = re.findall(r'<path[^>]*\sd="([^"]+)"', svg)[1]

# The mark-to-head transform and the nose centroid, as in build.mjs.
K = 0.0020374506788664233
NOSE_C = (750.48, 1052.89)


def head_r(x, y):
    n = len(HEAD_R)
    u = ((math.atan2(y, x) + math.pi) / (2 * math.pi)) * n
    i = int(math.floor(u)) % n
    f = u - math.floor(u)
    return HEAD_R[i] + (HEAD_R[(i + 1) % n] - HEAD_R[i]) * f


nose_lat = -round(math.degrees(math.asin(0.202 / HEAD_R[54])), 2)
eye_lift = round(math.degrees(math.asin(0.041 / HEAD_R[36])), 2)

nl = math.radians(nose_lat)
py0 = -math.sin(nl)
hk = head_r(0, py0)
ncx, ncy = 0.0, py0 * hk
sx, sy = K, K * math.cos(nl)


def transform(d, fx, fy):
    toks = re.findall(r"[MLHVCZmlhvcz]|-?\d*\.?\d+(?:e-?\d+)?", d)
    out, cur, i = "", None, 0
    arity = {"M": 2, "L": 2, "C": 6}
    while i < len(toks):
        if toks[i].isalpha():
            cur = toks[i]
            out += cur
            i += 1
            if cur in "Zz":
                continue
        if cur == "H":
            out += f"{fx(float(toks[i])):.5f} "
            i += 1
        elif cur == "V":
            out += f"{fy(float(toks[i])):.5f} "
            i += 1
        else:
            vals = [float(v) for v in toks[i : i + arity[cur]]]
            i += arity[cur]
            out += " ".join(f"{fx(v) if j % 2 == 0 else fy(v):.5f}" for j, v in enumerate(vals)) + " "
    return out.strip()


nose_t = transform(nose, lambda x: ncx + sx * (x - NOSE_C[0]), lambda y: ncy + sy * (y - NOSE_C[1]))

eyes = []
for side in (-1, 1):
    lon, el = math.radians(30 * side), math.radians(eye_lift)
    px0, py0 = math.sin(lon) * math.cos(el), -math.sin(el)
    k = head_r(px0, py0)
    eyes.append((px0 * k, py0 * k, max(math.cos(lon), 0) ** 0.4))

out = pathlib.Path(__file__).resolve().parent.parent / "Sources/CuaBotsCore/Avatar/KoalaGeometry.swift"
out.write_text(f'''// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Generated from the Cua koala design, derived from the Cua koala mark: the
// head blob and ear curve (the ear is a drawn fan), and the nose from the
// mark itself, placed for a front-facing head. Units are head
// radii with the head's centroid at the origin and y pointing down.
// Regenerate with `scripts/gen-koala-geometry.py <koala design source dir>`.

enum KoalaGeometry {{
    static let head = "{HEAD}"
    /// The right ear, centred on its own centroid; the left ear mirrors it.
    static let ear = "{EAR}"
    /// The right ear's centroid, from the head's centroid.
    static let earAt = (x: {EAR_AT[0]}, y: {EAR_AT[1]})
    static let nose = "{nose_t}"
    /// Eye centres (left, right) on the blob at 30 degrees of longitude, and
    /// the horizontal squash a glyph gets there.
    static let leftEye = (x: {eyes[0][0]:.5f}, y: {eyes[0][1]:.5f}, squash: {eyes[0][2]:.5f})
    static let rightEye = (x: {eyes[1][0]:.5f}, y: {eyes[1][1]:.5f}, squash: {eyes[1][2]:.5f})
}}
''')
print(f"wrote {out}")
