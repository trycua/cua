#!/usr/bin/env python3
"""Opt-in end-to-end test of the Omarchy image (libs/images/omarchy).

Boots the image through the cua CLI and checks that Omarchy's own Cua Driver
integration drives the Hyprland desktop:

* SDK/CLI path (``cua do``, cua-spacesd, input through the cua-hyprland-plugin):
  launch a terminal, click it (focus), type a command into it (the file it
  writes proves the keystrokes), and a Hyprland window action (resize).
* cua-driver MCP, twice: cua-spacesd's Streamable HTTP ``/mcp`` (``cua sb mcp``)
  and Omarchy's built-in ``cua-driver mcp`` (cua-driver-bin, over stdio in the
  guest), including a click through the built-in driver.
* The cua-driver cursor overlay, from both drivers: screenshots taken through
  the SDK right after each driver's pointer action (saved to --evidence), and
  the overlay's coloured arrow found in the pixels next to the click.
* Stream viewer input (``cua sb stream-probe``, a media session like the
  Spaces apps'): on the desktop stream and on a window stream, a click, typing
  and a scroll sent on the media socket are acknowledged as delivered and
  have their effect (focus, the typed file, the terminal's text moving). The
  viewer's click shows one cursor, the viewer's own: no agent participant
  joins presence for it and no agent cursor overlay is drawn at it.
* Stream frame rate while a terminal rewrites a counter nonstop: a window
  stream of a small window, asserted against --min-fps (the Omarchy release
  gate passes 12: Hyprland renders in software on a GPU-less KVM runner and
  holds 13-16 fps under this load; 0, the default, only reports it, for
  emulated guests), and the desktop
  stream under that load and under a scrolling terminal (reported; software
  rendering bounds it), each with Hyprland's and cua-spacesd's CPU and
  cua-spacesd's capture and encoder counters.

Opt-in: runs only with CUA_E2E_OMARCHY=1 (it starts a VM). Standard library only.

    CUA_E2E_OMARCHY=1 tests/e2e/omarchy/omarchy_e2e.py --disk ~/.cache/cua-images/omarchy/amd64/disk.img \\
        [--cua PATH] [--evidence DIR]
    # or against a machine that already runs the image:
    CUA_E2E_OMARCHY=1 tests/e2e/omarchy/omarchy_e2e.py --direct 127.0.0.1:3211 --token TOKEN
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import shlex
import shutil
import struct
import subprocess
import sys
import threading
import time
import zlib
from pathlib import Path

CUA = "cua"
NAME = ""
MIN_FPS = 0.0
RESULTS: list[tuple[str, bool, str]] = []


def log(msg: str) -> None:
    print(f"[omarchy-e2e {time.strftime('%H:%M:%S')}] {msg}", flush=True)


def cua(*args: str, timeout: float = 300, check: bool = True, stdin: str | None = None) -> str:
    cmd = [CUA, *args]
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, input=stdin)
    if check and p.returncode != 0:
        raise RuntimeError(f"{shlex.join(cmd)} -> {p.returncode}: {p.stderr.strip()[-800:]}")
    return p.stdout


def sh(command: str, timeout: float = 120, check: bool = True) -> str:
    """Run a shell line in the guest (cua sb exec, as the desktop user)."""
    return cua("sb", "exec", NAME, command, timeout=timeout, check=check)


def poll(what: str, fn, timeout: float, every: float = 3.0):
    deadline = time.monotonic() + timeout
    last: object = None
    while time.monotonic() < deadline:
        try:
            last = fn()
            if last:
                return last
        except Exception as e:  # noqa: BLE001 - keep polling; report the last error
            last = e
        time.sleep(every)
    raise TimeoutError(f"{what}: gave up after {timeout:.0f}s (last: {last!r})")


def check(name: str, fn) -> None:
    t0 = time.monotonic()
    try:
        detail = fn() or ""
        RESULTS.append((name, True, str(detail)))
        log(f"PASS {name} ({time.monotonic() - t0:.0f}s) {detail}")
    except Exception as e:  # noqa: BLE001
        RESULTS.append((name, False, str(e)))
        log(f"FAIL {name}: {e}")


def clients() -> list[dict]:
    return json.loads(sh("hyprctl -j clients"))


def window(app_id: str) -> dict | None:
    """The Hyprland client with this app id (class): shells rewrite titles."""
    return next((c for c in clients() if c.get("class") == app_id), None)


def active_class() -> str:
    return json.loads(sh("hyprctl -j activewindow") or "{}").get("class", "")


def target_of(w: dict) -> str:
    """The cua window id of a Hyprland client (matched by geometry)."""
    for line in cua("do", "window", "ls").splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0].startswith("target-"):
            box = line[line.rfind("[") + 1 : line.rfind("]")].split(",")
            if [int(float(v)) for v in box[:2]] == w["at"]:
                return parts[0]
    raise RuntimeError(f"no cua window at {w['at']}")


def shot(evidence: Path, name: str) -> Path:
    out = evidence / f"{name}.png"
    for attempt in range(3):  # a busy (or emulated) guest can miss one frame deadline
        try:
            cua("do", "screenshot", "--save", str(out), timeout=120)
            return out
        except RuntimeError:
            if attempt == 2:
                raise
            time.sleep(3)
    return out


def read_png_rgb(path: Path) -> tuple[int, int, list[bytearray]]:
    """Decode an 8-bit RGB/RGBA non-interlaced PNG (what screenshots are) to RGB rows."""
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos, idat, width = 8, b"", 0
    height = depth = color = interlace = 0
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind, body = data[pos + 4 : pos + 8], data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
    assert depth == 8 and color in (2, 6) and not interlace, (depth, color, interlace)
    bpp = 3 if color == 2 else 4
    raw, stride, rows = zlib.decompress(idat), width * bpp, []
    prev = bytearray(stride)
    for y in range(height):
        f, line = raw[y * (stride + 1)], bytearray(
            raw[y * (stride + 1) + 1 : (y + 1) * (stride + 1)]
        )
        for i in range(stride):
            a = line[i - bpp] if i >= bpp else 0
            b, c = prev[i], prev[i - bpp] if i >= bpp else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(bytearray(v for i, v in enumerate(line) if bpp == 3 or i % 4 != 3))
        prev = line
    return width, height, rows


def overlay_pixels_near(path: Path, x: float, y: float, screen_w: int, radius: int = 90) -> int:
    """Saturated pixels (the agent cursor's coloured arrow and badge) within
    `radius` screen points of (x, y). Omarchy's desktop, terminals and pointer
    are greyscale, so these are the overlay's."""
    width, height, rows = read_png_rgb(path)
    scale = width / screen_w
    cx, cy, r = int(x * scale), int(y * scale), int(radius * scale)
    count = 0
    for py in range(max(0, cy - r), min(height, cy + r)):
        row = rows[py]
        for px in range(max(0, cx - r), min(width, cx + r)):
            red, green, blue = row[3 * px : 3 * px + 3]
            if max(red, green, blue) - min(red, green, blue) > 90 and max(red, green, blue) > 120:
                count += 1
    return count


def overlay_shot(
    evidence: Path, name: str, x: float, y: float, timeout: float = 30, image_coords: bool = False
) -> str:
    """Screenshot until the agent cursor is drawn at (x, y), in desktop points
    or, with image_coords, in the screenshot's pixels (what `cua do click`
    takes). Pointer actions wait for the glide, but an emulated guest paints
    slowly, so allow a few frames to land."""
    deadline, overlay = time.monotonic() + timeout, 0
    for _ in range(20):
        path = shot(evidence, name)
        screen_w = read_png_rgb(path)[0] if image_coords else 1280
        overlay = overlay_pixels_near(path, x, y, screen_w=screen_w)
        if overlay >= 40 or time.monotonic() >= deadline:
            break
        time.sleep(2)
    assert overlay >= 40, f"no agent cursor overlay near ({x},{y}) ({overlay} coloured pixels)"
    return f"overlay {overlay} coloured px"


def main() -> int:
    global CUA, NAME, MIN_FPS
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument(
        "--disk", help="omarchy disk.img to boot with `cua sb create vm:<disk> --on local`"
    )
    ap.add_argument(
        "--image", help="image ref to create instead (e.g. ghcr.io/trycua/omarchy:edge)"
    )
    ap.add_argument("--direct", help="host:port of a running image's cua-spacesd instead of --disk")
    ap.add_argument("--token", help="spacesd token (default: generated for --disk)")
    ap.add_argument("--cua", default=shutil.which("cua") or "cua")
    ap.add_argument("--evidence", default="omarchy-e2e-evidence")
    ap.add_argument("--boot-timeout", type=float, default=2400)
    ap.add_argument("--keep", action="store_true", help="leave the sandbox running")
    ap.add_argument("--reuse", help="run the checks against this existing sandbox name")
    ap.add_argument(
        "--min-fps",
        type=float,
        default=0,
        help="desktop stream fps required under load (0: report only)",
    )
    a = ap.parse_args()
    if os.environ.get("CUA_E2E_OMARCHY") != "1":
        print("skipped: set CUA_E2E_OMARCHY=1 to boot the Omarchy VM", file=sys.stderr)
        return 0
    if not a.reuse and sum(map(bool, (a.disk, a.image, a.direct))) != 1:
        ap.error("pass exactly one of --disk, --image and --direct")
    CUA = a.cua
    MIN_FPS = a.min_fps
    evidence = Path(a.evidence)
    evidence.mkdir(parents=True, exist_ok=True)
    token = a.token or secrets.token_hex(16)
    NAME = f"cua-e2e-omarchy-{secrets.token_hex(3)}"

    if a.reuse:
        NAME = a.reuse
        cua("do", "switch", NAME)
        run_checks(evidence, a.boot_timeout)
        return finish(evidence)
    log(f"creating {NAME}")
    if a.disk or a.image:
        image = a.image or f"vm:{Path(a.disk).resolve()}"
        cua(
            "sb",
            "create",
            image,
            "--on",
            "local",
            "--kind",
            "vm",
            "--name",
            NAME,
            "--cpu",
            "4",
            "--memory",
            "4GB",
            "--port",
            "3211",
            "--wait",
            "tcp:3211",
            "--ready-timeout",
            str(int(a.boot_timeout)),
            "--token",
            token,
            "--env",
            f"CUA_ENV_TOKEN={token}",
            timeout=a.boot_timeout + 300,
        )
    else:
        cua(
            "sb",
            "create",
            "--on",
            f"direct:{a.direct}",
            "--name",
            NAME,
            "--token",
            token,
            timeout=300,
        )
    try:
        cua("do", "switch", NAME)
        run_checks(evidence, a.boot_timeout)
    finally:
        if not a.keep:
            cua("sb", "rm", "--force", NAME, timeout=600, check=False)
    return finish(evidence)


def finish(evidence: Path) -> int:
    ok = all(r[1] for r in RESULTS)
    (evidence / "results.json").write_text(
        json.dumps([{"check": n, "pass": p, "detail": d} for n, p, d in RESULTS], indent=2) + "\n"
    )
    log(f"{sum(r[1] for r in RESULTS)}/{len(RESULTS)} checks passed")
    return 0 if ok and RESULTS else 1


def run_checks(evidence: Path, boot_timeout: float) -> None:
    log("waiting for the Hyprland session")
    poll(
        "Hyprland session",
        lambda: sh(
            'test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" && hyprctl -j monitors', check=False
        )
        .strip()
        .startswith("["),
        boot_timeout,
        every=10,
    )

    def facts():
        versions = sh("pacman -Q omarchy-dev cua-driver-bin cua-hyprland-plugin hyprland").strip()
        (evidence / "versions.txt").write_text(versions + "\n")
        status = json.loads(sh("hyprctl -j cua:status"))
        (evidence / "cua-status.json").write_text(json.dumps(status, indent=2) + "\n")
        assert "cua-driver-bin" in versions and "cua-hyprland-plugin" in versions, versions
        return versions.replace("\n", ", ")

    check("omarchy edge packages and loaded cua-hyprland-plugin", facts)

    title = "cua-e2e-foot"  # the terminal's app id

    def launch():
        cua("do", "launch", "foot", "--app-id", title, timeout=120)
        w = poll("foot window", lambda: window(title), 180)
        return f"{w['address']} at {w['at']} size {w['size']}"

    check("SDK launch_app: foot on Hyprland", launch)

    def click():
        w = window(title)
        assert w, "no foot window"
        # Focus another window first, so the click is what focuses the terminal.
        cua("do", "launch", "foot", "--app-id", "cua-e2e-other", timeout=120)
        poll("other focused", lambda: active_class() == "cua-e2e-other", 60, 1)
        w = window(title)
        x, y = w["at"][0] + w["size"][0] // 2, w["at"][1] + w["size"][1] // 2
        cua("do", "click", str(x), str(y))
        overlay = overlay_shot(evidence, "overlay-sdk-click", x, y, image_coords=True)
        focused = poll(
            "foot focused",
            lambda: active_class() == title and f"focused {title} at ({x},{y})",
            30,
            1,
        )
        return f"{focused}; {overlay}"

    check("SDK click focuses the window", click)

    def type_text():
        marker = f"cua-e2e-typed-{secrets.token_hex(3)}"
        cua("do", "type", f"echo {marker} > /tmp/cua-e2e-typed")
        cua("do", "key", "enter")
        got = poll(
            "typed file",
            lambda: sh("cat /tmp/cua-e2e-typed 2>/dev/null", check=False).strip(),
            30,
            1,
        )
        assert got == marker, got
        shot(evidence, "typed")
        return got

    check("SDK type + key into foot", type_text)

    def window_action():
        before = window(title)
        assert before
        cua("do", "window", "maximize", target_of(before))
        after = poll(
            "maximized", lambda: (w := window(title)) and w["size"] != before["size"] and w, 30, 1
        )
        shot(evidence, "window-maximized")
        return f"size {before['size']} -> {after['size']}"

    check("SDK Hyprland window action (maximize)", window_action)

    def spacesd_mcp():
        tools = cua("sb", "mcp", NAME, "env", "tools", timeout=120)
        assert "click" in tools, tools[:400]
        size = cua("sb", "mcp", NAME, "env", "call", "get_screen_size", "{}", timeout=120)
        return f"{len(tools.splitlines())} tool lines; get_screen_size: {size.strip()[:120]}"

    check("cua-driver MCP via cua-spacesd /mcp", spacesd_mcp)

    def builtin_mcp():
        # Omarchy's own cua-driver (cua-driver-bin): its stdio MCP server,
        # piped transparently, lists its tools.
        req = "\n".join(
            json.dumps(m)
            for m in [
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "clientInfo": {"name": "cua-e2e-omarchy", "version": "1"},
                    },
                },
                {"jsonrpc": "2.0", "method": "notifications/initialized"},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            ]
        )
        out = sh(
            f"printf '%s\\n' {shlex.quote(req)} | timeout 60 /usr/bin/cua-driver mcp", timeout=120
        )
        lines = [json.loads(line) for line in out.splitlines() if line.startswith("{")]
        tools = next(line for line in lines if line.get("id") == 2)["result"]["tools"]
        names = {t["name"] for t in tools}
        assert "click" in names, sorted(names)[:20]
        return f"/usr/bin/cua-driver mcp: {len(names)} tools"

    check("Omarchy built-in cua-driver MCP (stdio)", builtin_mcp)

    def builtin_click():
        # Omarchy's daemon (cua-driver.service) clicks in desktop coordinates;
        # Hyprland's pointer must end up there, with the daemon's own agent
        # cursor drawn at the click. The call names its session: an anonymous
        # one-shot `cua-driver call` ends its session (and removes its cursor)
        # as soon as it returns, the way an MCP client's session does not.
        x, y = 900, 300
        args = json.dumps({"x": x, "y": y, "scope": "desktop", "session": "cua-e2e-omarchy"})
        out = json.loads(sh(f"/usr/bin/cua-driver call click '{args}'", timeout=120))
        assert "code" not in out, out
        pos = json.loads(sh("hyprctl -j cursorpos"))
        assert (round(pos["x"]), round(pos["y"])) == (x, y), pos
        overlay = overlay_shot(evidence, "builtin-driver-click", x, y)
        # End the session: its cursor hides at once, so the stream checks
        # below see only what a viewer's input draws.
        sh(
            f"/usr/bin/cua-driver call end_session '{json.dumps({'session': 'cua-e2e-omarchy'})}'",
            timeout=60,
            check=False,
        )
        return f"route {out.get('route')}; Hyprland pointer at ({x},{y}); {overlay}"

    check("Omarchy built-in cua-driver click", builtin_click)

    stream_checks(evidence, title)


def probe(evidence: Path, name: str, *args: str, seconds: float = 2) -> dict:
    """`cua sb stream-probe`: a media session like a Spaces viewer's, with
    input sent as interactive_input batches on the media socket. The report
    is saved as evidence."""
    out = cua(
        "sb",
        "stream-probe",
        NAME,
        "--seconds",
        str(seconds),
        "--pings",
        "5",
        "--action-timeout",
        "180",
        *args,
        timeout=1500,
    )
    report = json.loads(out.strip().splitlines()[-1])
    (evidence / f"{name}.json").write_text(json.dumps(report, indent=2) + "\n")
    assert report["width"] and report["height"], report
    assert report["ping"]["answered"] == report["ping"]["sent"], report["ping"]
    refused = [a for a in report["actions"] if "delivered" in a and not a["delivered"]]
    assert not refused, f"input not delivered: {refused}"
    return report


def region_changed(
    before: Path,
    after: Path,
    box: tuple[int, int, int, int],
    skip: tuple[float, float],
    screen_w: int,
) -> int:
    """Pixels that differ inside `box` (x, y, w, h in desktop points) between
    two screenshots, leaving out 120 points around `skip` (where the agent
    cursor is drawn)."""
    wb, hb, a = read_png_rgb(before)
    wa, ha, b = read_png_rgb(after)
    assert (wb, hb) == (wa, ha), "screenshot size changed"
    s = wb / screen_w
    x0, y0, x1, y1 = (
        int(box[0] * s),
        int(box[1] * s),
        int((box[0] + box[2]) * s),
        int((box[1] + box[3]) * s),
    )
    sx, sy, r = skip[0] * s, skip[1] * s, 120 * s
    count = 0
    for py in range(max(0, y0), min(hb, y1)):
        for px in range(max(0, x0), min(wb, x1)):
            if abs(px - sx) < r and abs(py - sy) < r:
                continue
            if max(abs(a[py][3 * px + c] - b[py][3 * px + c]) for c in range(3)) > 40:
                count += 1
    return count


def stream_checks(evidence: Path, title: str) -> None:
    """Input from stream viewers (the Spaces apps and the HTML5 viewer): the
    desktop stream and a window stream each deliver a click, typing and a
    scroll through the media session, and each has a visible effect."""
    mon = json.loads(sh("hyprctl -j monitors"))[0]
    screen = (mon["width"] / mon["scale"], mon["height"] / mon["scale"])

    def norm(x: float, y: float) -> str:
        return f"{x / screen[0]:.4f},{y / screen[1]:.4f}"

    def fill_terminal() -> dict:
        """Numbered lines in the focused terminal, so a scroll moves text."""
        cua("do", "type", "clear; seq 1 400")
        cua("do", "key", "enter")
        time.sleep(3)
        w = window(title)
        assert w
        return w

    def scrolled(name: str, w: dict, scroll_args: list[str], point: tuple[float, float]) -> str:
        before = shot(evidence, f"{name}-before-scroll")
        report = probe(evidence, f"{name}-scroll", *scroll_args)
        time.sleep(2)
        after = shot(evidence, f"{name}-after-scroll")
        n = region_changed(before, after, (*w["at"], *w["size"]), point, int(screen[0]))
        assert n > 500, f"the scroll changed {n} pixels of the terminal"
        return f"ack {report['actions'][0].get('ack_ms')} ms; {n} px changed"

    def desktop_click():
        # The window action above maximized the terminal; tile it again so
        # the other terminal cannot cover the point clicked.
        w = window(title)
        if w and w.get("fullscreen"):
            cua("do", "window", "restore", target_of(w), check=False)
            try:
                poll("foot tiled", lambda: not (window(title) or {}).get("fullscreen"), 15, 1)
            except TimeoutError:
                sh(
                    f"hyprctl dispatch focuswindow address:{w['address']}; hyprctl dispatch fullscreen 1",
                    check=False,
                )
                poll("foot tiled", lambda: not (window(title) or {}).get("fullscreen"), 30, 1)
        cua("do", "launch", "foot", "--app-id", "cua-e2e-other2", timeout=120)
        poll("other focused", lambda: active_class() == "cua-e2e-other2", 60, 1)
        time.sleep(2)  # the layout animation settles
        w = window(title)
        assert w
        x, y = w["at"][0] + w["size"][0] // 2, w["at"][1] + w["size"][1] // 2
        covering = [
            c["class"]
            for c in clients()
            if c.get("class") != title
            and c.get("mapped", True)
            and c["at"][0] <= x < c["at"][0] + c["size"][0]
            and c["at"][1] <= y < c["at"][1] + c["size"][1]
        ]
        assert not covering, f"({x},{y}) is also inside {covering}"
        report = probe(
            evidence, "stream-desktop-click", "--action", f"click:{norm(x, y)}", "--presence-check"
        )
        focused = poll("foot focused", lambda: active_class() == title, 60, 1)
        a = report["actions"][0]
        # A viewer's click shows one cursor, the viewer's own: no agent joins
        # presence for it and no agent cursor overlay is drawn at it.
        assert report["presence"]["new_agents"] == [], report["presence"]
        after = shot(evidence, "stream-desktop-click-after")
        overlay = overlay_pixels_near(after, x, y, screen_w=int(screen[0]))
        assert (
            overlay < 40
        ), f"an agent cursor overlay at the viewer's click ({overlay} coloured px)"
        return (
            f"{report['codec']} {report['width']}x{report['height']}, first frame "
            f"{report['first_frame_ms']} ms, ping {report['ping']['median_ms']} ms; "
            f"click ack {a.get('ack_ms')} ms; focused {focused}; no agent cursor "
            f"({overlay} coloured px, agents {report['presence']['agents_after']})"
        )

    check("desktop stream: click focuses the window", desktop_click)

    def desktop_type():
        marker = f"cua-e2e-stream-{secrets.token_hex(3)}"
        report = probe(
            evidence,
            "stream-desktop-type",
            "--action",
            f"type:echo {marker} > /tmp/cua-e2e-stream-typed",
            "--action",
            "key:enter",
        )
        got = poll(
            "typed file",
            lambda: sh("cat /tmp/cua-e2e-stream-typed 2>/dev/null", check=False).strip(),
            60,
            1,
        )
        assert got == marker, got
        return f"{got}; acks {[a.get('ack_ms') for a in report['actions']]} ms"

    check("desktop stream: type + key into foot", desktop_type)

    def desktop_scroll():
        w = fill_terminal()
        x, y = w["at"][0] + w["size"][0] // 2, w["at"][1] + w["size"][1] // 2
        return scrolled("stream-desktop", w, ["--action", f"scroll:{norm(x, y)},-5"], (x, y))

    check("desktop stream: scroll moves the terminal", desktop_scroll)

    target = target_of(window(title))

    def window_type():
        marker = f"cua-e2e-window-{secrets.token_hex(3)}"
        report = probe(
            evidence,
            "stream-window-type",
            "--window",
            target,
            "--action",
            "click:0.5,0.5",
            "--action",
            f"type:echo {marker} > /tmp/cua-e2e-window-typed",
            "--action",
            "key:enter",
        )
        got = poll(
            "typed file",
            lambda: sh("cat /tmp/cua-e2e-window-typed 2>/dev/null", check=False).strip(),
            60,
            1,
        )
        assert got == marker, got
        return (
            f"{got}; policy {report['policy']}; "
            f"acks {[a.get('ack_ms') for a in report['actions']]} ms"
        )

    check("window stream: click, type + key into foot", window_type)

    def window_scroll():
        w = fill_terminal()
        x, y = w["at"][0] + w["size"][0] // 2, w["at"][1] + w["size"][1] // 2
        return scrolled(
            "stream-window", w, ["--window", target, "--action", "scroll:0.5,0.5,-5"], (x, y)
        )

    check("window stream: scroll moves the terminal", window_scroll)

    def cpu_during(seconds: int) -> dict:
        """% of one core used by Hyprland and cua-spacesd over `seconds`."""
        script = (
            "pids() { pgrep -x Hyprland | sed 's/^/Hyprland /'; "
            "for p in $(pgrep -x cua-spacesd); do "
            'grep -q token-sync /proc/$p/cmdline || echo "cua-spacesd $p"; done; }; '
            "snap() { pids | while read n p; do echo \"$n $(awk '{print $14+$15}' /proc/$p/stat)\"; done; }; "
            f"snap; echo --; sleep {seconds}; snap; getconf CLK_TCK"
        )
        out = sh(script, timeout=seconds + 60, check=False).split()
        try:
            sep, hz = out.index("--"), int(out[-1])
            before = dict(zip(out[0:sep:2], map(int, out[1:sep:2])))
            after = dict(zip(out[sep + 1 : -1 : 2], map(int, out[sep + 2 : -1 : 2])))
            return {
                k: round((after[k] - before[k]) / hz / seconds * 100) for k in after if k in before
            }
        except (ValueError, IndexError):
            return {}

    def fps_under(name: str, loop: str, size: tuple[int, int] | None = None) -> tuple[float, str]:
        """Stream fps while a terminal runs `loop`, with CPU use: the desktop
        stream, or with `size` a window stream of the terminal floated at
        that size."""
        app = f"cua-e2e-{name}"
        cua("do", "launch", "foot", "--app-id", app, "sh", "-c", loop, timeout=120)
        poll(f"{name} terminal", lambda: window(app), 60, 1)
        time.sleep(3)
        args: list[str] = []
        if size:
            target = target_of(window(app))
            cua("do", "window", "resize", target, str(size[0]), str(size[1]))
            poll(
                f"{name} resized",
                lambda: all(
                    abs(a - b) <= 24 for a, b in zip((window(app) or {}).get("size", [0, 0]), size)
                ),
                30,
                1,
            )
            time.sleep(2)
            args = ["--window", target]
        cpu: dict = {}
        sampler = threading.Thread(target=lambda: cpu.update(cpu_during(6)))
        try:
            sampler.start()
            report = probe(evidence, f"stream-fps-{name}", *args, seconds=10)
        finally:
            sampler.join()
            w = window(app)
            if w and int(w.get("pid", 0)) > 0:
                sh(f"kill {int(w['pid'])}", check=False)
        cpu_text = ", ".join(f"{k} {v}%" for k, v in sorted(cpu.items()))
        return report["fps"], (
            f"{report['fps']} fps at {report['width']}x{report['height']} "
            f"({report['frames']} frames; {cpu_text})"
        )

    def stream_fps():
        # The capture skips unchanged frames, so an idle desktop is not a
        # load; a terminal rewriting a counter in place changes every frame
        # (bash, as /bin/sh on Arch, has $EPOCHREALTIME).
        # Hyprland renders in software here and its screencopy readback of
        # the whole 1280x800 output bounds the desktop stream (reported).
        # A small window's stream is not bounded by the compositor, so it
        # measures what capture and encoding keep up with (asserted).
        # About 100 updates a second without forking a process per update,
        # so the load itself does not saturate the guest.
        counter = "while :; do printf '\\r%s ' \"$EPOCHREALTIME\"; sleep 0.01; done"
        since = sh("date '+%Y-%m-%d %H:%M:%S'").strip()
        window_fps, window_text = fps_under("fps-window", counter, size=(480, 270))
        _, desktop_text = fps_under("fps-desktop", counter)
        _, scroll_text = fps_under("fps-scroll", "while :; do date +%s.%N; done")
        # cua-spacesd logs each stream's capture and encoder counters when
        # it ends: they place a low frame rate (compositor, capture, encoder).
        journal = sh(
            f"(sudo -n journalctl -u cua-spacesd --since '{since}' -o cat 2>/dev/null "
            f"|| journalctl -u cua-spacesd --since '{since}' -o cat) "
            "| grep -E 'wayland capture ended|encoder stopped' | tail -12",
            check=False,
        )
        (evidence / "stream-fps-journal.txt").write_text(journal)
        pipeline = " | ".join(line.strip() for line in journal.splitlines())
        detail = (
            f"window (counter) {window_text}; desktop (counter) {desktop_text}; "
            f"desktop (scrolling) {scroll_text}; min {MIN_FPS}; pipeline: {pipeline}"
        )
        assert (
            window_fps >= MIN_FPS
        ), f"window stream {window_fps} fps under load, want >= {MIN_FPS}; {detail}"
        return detail

    check("stream frame rate under load", stream_fps)


if __name__ == "__main__":
    sys.exit(main())
