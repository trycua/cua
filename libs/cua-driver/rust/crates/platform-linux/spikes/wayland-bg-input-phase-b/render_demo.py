#!/usr/bin/env python3
# Render the 16 per-window received pointer paths into a cursive-stroke montage.
import re, colorsys, glob
from PIL import Image, ImageDraw, ImageFont

TILE_W, TILE_H, PAD, COLS = 230, 150, 10, 4
TITLE_H = 64
mot = re.compile(r"motion:.*x, y: ([0-9.]+), ([0-9.]+)")

def load(path):
    pts = []
    try:
        for line in open(path, errors="replace"):
            m = mot.search(line)
            if m: pts.append((float(m.group(1)), float(m.group(2))))
    except FileNotFoundError:
        pass
    return pts

logs = sorted(glob.glob("/tmp/ink-*.log"), key=lambda p: int(re.search(r"(\d+)", p).group(1)))
n = len(logs)
rows = (n + COLS - 1) // COLS
W = COLS * TILE_W + (COLS + 1) * PAD
H = TITLE_H + rows * TILE_H + (rows + 1) * PAD
img = Image.new("RGB", (W, H), (18, 18, 22))
d = ImageDraw.Draw(img)
try:
    fbig = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 20)
    fsm = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 12)
except Exception:
    fbig = fsm = ImageFont.load_default()

d.text((PAD, 12), "cua-driver on Wayland — 16 background windows, 16 independent libei cursors, concurrent cursive",
       fill=(235, 235, 240), font=fbig)
d.text((PAD, 40), "no window focused or raised · no real pointer moved · libei client → EIS server in compositor → direct per-surface injection",
       fill=(150, 200, 255), font=fsm)

drawn = 0
for i, path in enumerate(logs):
    r, c = divmod(i, COLS)
    ox = PAD + c * (TILE_W + PAD)
    oy = TITLE_H + PAD + r * (TILE_H + PAD)
    d.rectangle([ox, oy, ox + TILE_W, oy + TILE_H], fill=(255, 255, 255), outline=(70, 70, 80))
    pts = load(path)
    label = "win %02d  (%d pts)" % (i, len(pts))
    if len(pts) >= 2:
        xs = [p[0] for p in pts]; ys = [p[1] for p in pts]
        minx, maxx, miny, maxy = min(xs), max(xs), min(ys), max(ys)
        sw, sh = maxx - minx or 1, maxy - miny or 1
        m = 18
        sc = min((TILE_W - 2*m) / sw, (TILE_H - 2*m - 12) / sh)
        rgb = tuple(int(255*v) for v in colorsys.hsv_to_rgb(i / max(n,1), 0.85, 0.9))
        sp = [(ox + m + (x - minx) * sc, oy + m + 12 + (y - miny) * sc) for x, y in pts]
        d.line(sp, fill=rgb, width=3, joint="curve")
        drawn += 1
    d.text((ox + 6, oy + 4), label, fill=(40, 40, 40), font=fsm)

img.save("/tmp/cursive_montage.png")
print("rendered %d/%d windows -> /tmp/cursive_montage.png  (%dx%d)" % (drawn, n, W, H))
