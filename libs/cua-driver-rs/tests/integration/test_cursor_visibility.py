"""Integration test: agent-cursor custom-icon visibility on screen.

Verifies that after:
  1. Setting a custom cursor icon via set_agent_cursor_style
  2. Performing a click (which triggers ClickPulse moving the cursor to the
     click point and PinAbove ordering the overlay above the target window)

... the custom cursor icon is visually present at the expected screen position
in a full-screen screenshot.

The test cursor is a bright-magenta circle with a yellow cross — chosen because
it's easy to detect via colour thresholding without needing a heavy CV library.
Only the stdlib + base64/struct/zlib are needed for the screenshot decode.

Run:
    CUA_DRIVER_BINARY=../../target/debug/cua-driver python3 -m unittest test_cursor_visibility -v
"""

from __future__ import annotations

import base64
import os
import struct
import subprocess
import sys
import time
import unittest
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path, resolve_window_id

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_FIXTURE_CURSOR = os.path.join(_THIS_DIR, "fixtures", "test_cursor.png")

CALCULATOR_BUNDLE = "com.apple.calculator"

# Colour tolerances for detection
_MAGENTA_MIN = (200, 0, 200)   # R≥200, G≤55, B≥200
_MAGENTA_MAX = (255, 55, 255)
_YELLOW_MIN  = (200, 200, 0)   # R≥200, G≥200, B≤55
_YELLOW_MAX  = (255, 255, 55)

# Minimum fraction of pixels in the search region that must match the cursor colours.
_MATCH_THRESHOLD = 0.002  # 0.2% of the search window


def _decode_png_pixels(png_bytes: bytes) -> tuple[int, int, list[tuple[int, int, int]]]:
    """Minimal PNG decoder: returns (width, height, [(r,g,b), ...]) for RGB/RGBA PNGs.

    Only supports filter types 0 (None) and 1 (Sub) — which covers the screenshots
    produced by cua-driver's `screenshot` tool. Raises ValueError for unsupported formats.
    """
    if png_bytes[:8] != b'\x89PNG\r\n\x1a\n':
        raise ValueError("Not a PNG file")

    pos = 8
    width = height = 0
    idat_chunks: list[bytes] = []
    bit_depth = color_type = 0

    while pos < len(png_bytes):
        if pos + 8 > len(png_bytes):
            break
        length = struct.unpack('>I', png_bytes[pos:pos+4])[0]
        chunk_type = png_bytes[pos+4:pos+8]
        data = png_bytes[pos+8:pos+8+length]
        pos += 12 + length

        if chunk_type == b'IHDR':
            width, height = struct.unpack('>II', data[:8])
            bit_depth = data[8]
            color_type = data[9]
        elif chunk_type == b'IDAT':
            idat_chunks.append(data)
        elif chunk_type == b'IEND':
            break

    if width == 0 or height == 0:
        raise ValueError("No IHDR found")
    if bit_depth != 8:
        raise ValueError(f"Unsupported bit depth: {bit_depth}")

    raw = zlib.decompress(b''.join(idat_chunks))

    # Channels: 2=RGB(3), 6=RGBA(4)
    channels = {2: 3, 6: 4}.get(color_type)
    if channels is None:
        raise ValueError(f"Unsupported color_type: {color_type}")

    stride = width * channels
    pixels: list[tuple[int, int, int]] = []
    prev_row = bytes(stride)

    offset = 0
    for _y in range(height):
        filt = raw[offset]
        offset += 1
        row = bytearray(raw[offset:offset+stride])
        offset += stride

        if filt == 0:
            pass  # None
        elif filt == 1:
            for i in range(channels, stride):
                row[i] = (row[i] + row[i - channels]) & 0xFF
        elif filt == 2:
            for i in range(stride):
                row[i] = (row[i] + prev_row[i]) & 0xFF
        elif filt == 3:
            for i in range(stride):
                a = row[i - channels] if i >= channels else 0
                b = prev_row[i]
                row[i] = (row[i] + (a + b) // 2) & 0xFF
        elif filt == 4:
            for i in range(stride):
                a = row[i - channels] if i >= channels else 0
                b = prev_row[i]
                c = prev_row[i - channels] if i >= channels else 0
                p = a + b - c
                pa = abs(p - a); pb = abs(p - b); pc = abs(p - c)
                pr = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                row[i] = (row[i] + pr) & 0xFF
        else:
            raise ValueError(f"Unknown PNG filter type: {filt}")

        for i in range(0, stride, channels):
            pixels.append((row[i], row[i+1], row[i+2]))
        prev_row = bytes(row)

    return width, height, pixels


def _colour_in_region(
    pixels: list[tuple[int, int, int]],
    img_w: int,
    img_h: int,
    cx: float,
    cy: float,
    radius: int,
    colour_min: tuple[int, int, int],
    colour_max: tuple[int, int, int],
) -> float:
    """Return fraction of pixels in circle around (cx, cy) that match the colour range."""
    x0 = max(0, int(cx) - radius)
    x1 = min(img_w - 1, int(cx) + radius)
    y0 = max(0, int(cy) - radius)
    y1 = min(img_h - 1, int(cy) + radius)

    total = 0
    matches = 0
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            idx = y * img_w + x
            if idx >= len(pixels):
                continue
            r, g, b = pixels[idx]
            total += 1
            if (colour_min[0] <= r <= colour_max[0] and
                    colour_min[1] <= g <= colour_max[1] and
                    colour_min[2] <= b <= colour_max[2]):
                matches += 1

    return matches / total if total > 0 else 0.0


class TestCursorVisibility(unittest.TestCase):
    """Custom cursor icon is visible on screen after a click."""

    @classmethod
    def setUpClass(cls) -> None:
        if not os.path.exists(_FIXTURE_CURSOR):
            cls.skipTest(cls, f"test cursor fixture missing: {_FIXTURE_CURSOR}")  # type: ignore[arg-type]
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        time.sleep(0.3)

    @classmethod
    def tearDownClass(cls) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)

    def setUp(self) -> None:
        self.client = DriverClient(default_binary_path()).__enter__()
        # Verify set_agent_cursor_style is available.
        tools = {t["name"] for t in self.client.list_tools()}
        if "set_agent_cursor_style" not in tools:
            self.client.__exit__(None, None, None)
            self.skipTest("set_agent_cursor_style tool not available in this build")

    def tearDown(self) -> None:
        subprocess.run(["pkill", "-x", "Calculator"], check=False)
        self.client.__exit__(None, None, None)

    def test_custom_cursor_visible_after_click(self) -> None:
        """After set_agent_cursor_style + click, the custom icon is on-screen."""
        client = self.client

        # 1. Enable cursor overlay.
        r = client.call_tool("set_agent_cursor_enabled", {"enabled": True})
        self.assertFalse(r.get("isError"), f"set_agent_cursor_enabled failed: {r}")

        # 2. Upload the bright-magenta test cursor.
        r = client.call_tool("set_agent_cursor_style", {"image_path": _FIXTURE_CURSOR})
        self.assertFalse(r.get("isError"), f"set_agent_cursor_style failed: {r}")
        text = r.get("content", [{}])[0].get("text", "")
        self.assertIn("✅", text, f"set_agent_cursor_style didn't return ✅: {text}")
        self.assertIn(_FIXTURE_CURSOR, text, f"image_path not echoed in response: {text}")

        # 3. Launch Calculator in the background.
        r = client.call_tool("launch_app", {"bundle_id": CALCULATOR_BUNDLE})
        self.calc_pid = r["structuredContent"]["pid"]
        time.sleep(0.6)

        # 4. Resolve the Calculator window and its screen bounds.
        win_id = resolve_window_id(client, self.calc_pid)
        wins = client.call_tool("list_windows", {"pid": self.calc_pid})["structuredContent"]["windows"]
        win = next((w for w in wins if w["window_id"] == win_id), None)
        self.assertIsNotNone(win, "Could not find Calculator window")
        bounds = win["bounds"]

        # 5. Click the center of the Calculator window (window-local coords).
        click_x = bounds["width"] / 2.0
        click_y = bounds["height"] / 2.0
        r = client.call_tool("click", {
            "pid": self.calc_pid,
            "window_id": win_id,
            "x": click_x,
            "y": click_y,
        })
        self.assertFalse(r.get("isError"), f"click failed: {r}")

        # 6. Compute expected screen coordinates of the click point.
        # The driver translates window-local coords to screen coords by adding
        # the window origin and dividing by the Retina scale (detected at click time).
        # We use the same heuristic: try a scale of 2.0 (Retina), fall back to 1.0.
        # If the click lands within ±64 px of our estimate it's still detectable.
        win_x = bounds["x"]
        win_y = bounds["y"]
        # Best-effort Retina scale detection via screenshot width vs logical width.
        try:
            scr = client.call_tool("screenshot", {"pid": self.calc_pid, "window_id": win_id})
            b64 = scr.get("content", [{}])[0].get("data", "")
            if b64:
                png = base64.b64decode(b64)
                # PNG width is at bytes 16..20 in big-endian.
                png_w = struct.unpack('>I', png[16:20])[0]
                scale = png_w / bounds["width"] if bounds["width"] > 0 else 1.0
            else:
                scale = 1.0
        except Exception:
            scale = 1.0

        screen_x = win_x + click_x / scale
        screen_y = win_y + click_y / scale

        # 7. Wait for cursor animation + render loop to settle.
        time.sleep(0.5)

        # 8. Take a full-screen screenshot.
        scr = client.call_tool("screenshot", {})
        b64 = scr.get("content", [{}])[0].get("data", "")
        self.assertTrue(b64, "screenshot returned no data")
        png_bytes = base64.b64decode(b64)

        # 9. Decode the PNG and check for our cursor colours near (screen_x, screen_y).
        try:
            img_w, img_h, pixels = _decode_png_pixels(png_bytes)
        except Exception as e:
            self.skipTest(f"PNG decode failed (unsupported screenshot format): {e}")

        # The screenshot may be at 2x resolution; scale screen_x/y accordingly.
        ss_scale = img_w / _get_screen_width(client)
        sx = screen_x * ss_scale
        sy = screen_y * ss_scale
        search_r = int(80 * ss_scale)

        magenta_frac = _colour_in_region(
            pixels, img_w, img_h, sx, sy, search_r,
            _MAGENTA_MIN, _MAGENTA_MAX,
        )
        yellow_frac = _colour_in_region(
            pixels, img_w, img_h, sx, sy, search_r,
            _YELLOW_MIN, _YELLOW_MAX,
        )

        self.assertGreater(
            magenta_frac, _MATCH_THRESHOLD,
            f"Magenta cursor colour NOT found near click point "
            f"(screen={screen_x:.0f},{screen_y:.0f} img={sx:.0f},{sy:.0f} "
            f"scale={ss_scale:.2f}). "
            f"Magenta fraction={magenta_frac:.5f} (threshold={_MATCH_THRESHOLD}). "
            f"Yellow fraction={yellow_frac:.5f}. "
            f"The overlay may be hidden or behind another window.",
        )
        self.assertGreater(
            yellow_frac, _MATCH_THRESHOLD,
            f"Yellow cross NOT found in cursor region "
            f"(magenta={magenta_frac:.5f}, yellow={yellow_frac:.5f}). "
            f"Cursor shape may not have loaded correctly.",
        )

    def test_default_cursor_visible_after_click(self) -> None:
        """After a plain click (no custom icon), the default gradient arrow is on-screen.

        Detects the ice-blue bloom/arrow colour (cursor_mid ≈ R94 G192 B232) that the
        built-in gradient arrow uses.  Does NOT set a custom cursor icon.
        """
        client = self.client

        # 1. Enable overlay with default cursor (no custom icon).
        r = client.call_tool("set_agent_cursor_enabled", {"enabled": True})
        self.assertFalse(r.get("isError"), f"set_agent_cursor_enabled failed: {r}")

        # Ensure no custom icon is active (revert to default arrow).
        r = client.call_tool("set_agent_cursor_style", {"image_path": ""})
        self.assertFalse(r.get("isError"), f"revert to default cursor failed: {r}")

        # 2. Launch Calculator in the background.
        r = client.call_tool("launch_app", {"bundle_id": CALCULATOR_BUNDLE})
        self.calc_pid = r["structuredContent"]["pid"]
        time.sleep(0.6)

        # 3. Resolve window and screen bounds.
        win_id = resolve_window_id(client, self.calc_pid)
        wins = client.call_tool("list_windows", {"pid": self.calc_pid})["structuredContent"]["windows"]
        win = next((w for w in wins if w["window_id"] == win_id), None)
        self.assertIsNotNone(win, "Could not find Calculator window")
        bounds = win["bounds"]

        click_x = bounds["width"] / 2.0
        click_y = bounds["height"] / 2.0

        # Detect Retina scale.
        try:
            scr = client.call_tool("screenshot", {"pid": self.calc_pid, "window_id": win_id})
            b64 = scr.get("content", [{}])[0].get("data", "")
            png = base64.b64decode(b64) if b64 else b""
            png_w = struct.unpack('>I', png[16:20])[0] if len(png) >= 24 else 0
            scale = png_w / bounds["width"] if bounds["width"] > 0 and png_w > 0 else 1.0
        except Exception:
            scale = 1.0

        # 4. Click the centre of the window.
        r = client.call_tool("click", {
            "pid": self.calc_pid,
            "window_id": win_id,
            "x": click_x,
            "y": click_y,
        })
        self.assertFalse(r.get("isError"), f"click failed: {r}")

        # 5. Wait for 750ms glide + spring settle + a safety margin.
        time.sleep(1.2)

        # 6. Full-screen screenshot.
        scr = client.call_tool("screenshot", {})
        b64 = scr.get("content", [{}])[0].get("data", "")
        self.assertTrue(b64, "screenshot returned no data")
        png_bytes = base64.b64decode(b64)

        try:
            img_w, img_h, pixels = _decode_png_pixels(png_bytes)
        except Exception as e:
            self.skipTest(f"PNG decode failed: {e}")

        win_x, win_y = bounds["x"], bounds["y"]
        screen_x = win_x + click_x / scale
        screen_y = win_y + click_y / scale

        ss_scale = img_w / _get_screen_width(client)
        sx = screen_x * ss_scale
        sy = screen_y * ss_scale
        search_r = int(80 * ss_scale)

        # Default arrow: ice-blue/cyan — cursor_mid ≈ (94, 192, 232), bloom ≈ (188, 232, 252).
        # Accept any pixel that is clearly blue-cyan (G and B significantly exceed R).
        _CYAN_MIN = (40,  150, 180)
        _CYAN_MAX = (180, 255, 255)

        cyan_frac = _colour_in_region(
            pixels, img_w, img_h, sx, sy, search_r,
            _CYAN_MIN, _CYAN_MAX,
        )

        self.assertGreater(
            cyan_frac, _MATCH_THRESHOLD,
            f"Default ice-blue cursor NOT found near click point "
            f"(screen={screen_x:.0f},{screen_y:.0f} img_pt={sx:.0f},{sy:.0f} "
            f"scale={ss_scale:.2f}). "
            f"Cyan fraction={cyan_frac:.5f} (threshold={_MATCH_THRESHOLD}). "
            f"The default gradient arrow overlay may be invisible.",
        )

    def test_set_agent_cursor_style_gradient_only(self) -> None:
        """set_agent_cursor_style with gradient_colors updates without error."""
        r = self.client.call_tool("set_agent_cursor_style", {
            "gradient_colors": ["#FF0000", "#FF00FF", "#0000FF"],
            "bloom_color": "#00FFFF",
        })
        self.assertFalse(r.get("isError"), f"gradient-only style update failed: {r}")
        text = r.get("content", [{}])[0].get("text", "")
        self.assertIn("✅", text)
        self.assertIn("#FF0000", text)
        self.assertIn("#00FFFF", text)

    def test_set_agent_cursor_style_revert(self) -> None:
        """Empty image_path reverts the cursor to the procedural arrow without error."""
        # First set a custom icon.
        self.client.call_tool("set_agent_cursor_style", {"image_path": _FIXTURE_CURSOR})
        # Then revert.
        r = self.client.call_tool("set_agent_cursor_style", {"image_path": ""})
        self.assertFalse(r.get("isError"), f"revert failed: {r}")
        text = r.get("content", [{}])[0].get("text", "")
        self.assertIn("✅", text)
        self.assertIn("reverted", text)


def _get_screen_width(client: DriverClient) -> float:
    """Return the logical screen width in points."""
    r = client.call_tool("get_screen_size")
    return float(r["structuredContent"].get("width", 1920))


if __name__ == "__main__":
    unittest.main(verbosity=2)
