#!/usr/bin/env python3
"""
Overlay Agent Cursor - A transparent floating cursor indicator
Runs inside the container and shows where the agent is clicking

Usage:
  overlay-cursor.py [--name=NAME] [--color=HEX]

Environment variables:
  CUABOT_NAME  - Session name (default: cuabot)
  CUABOT_COLOR - Hex color without # (default: derived from name)
"""

import json
import math
import os
import random
import socket
import sys
import threading
import time

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("GdkPixbuf", "2.0")
import cairo
from gi.repository import Gdk, GdkPixbuf, GLib, Gtk

# Configuration
SOCKET_PATH = "/tmp/cuabot-overlay-cursor.sock"
CURSOR_SIZE = 32
FADE_DELAY = 15.0  # seconds before fade starts
FADE_DURATION = 1.0  # seconds to fade out
MOVE_DURATION = 0.1  # seconds to move
SPAWN_DISTANCE = 128  # pixels from initial point

# Debug: show masked value and frame counter as a label
SHOW_MASKED_DEBUG = False


def name_to_color(name: str) -> str:
    """Generate a consistent hex color from a name (matches TypeScript version)"""
    hash_val = 0
    for char in name:
        hash_val = ((hash_val << 5) - hash_val) + ord(char)
        hash_val = hash_val & 0xFFFFFFFF  # Convert to 32-bit integer

    # Use golden ratio for nice hue distribution
    hue = (abs(hash_val) * 0.618033988749895) % 1

    # Convert HSL to RGB (saturation=0.7, lightness=0.5)
    s = 0.7
    lightness = 0.5

    c = (1 - abs(2 * lightness - 1)) * s
    x = c * (1 - abs((hue * 6) % 2 - 1))
    m = lightness - c / 2

    h = hue * 6
    if h < 1:
        r, g, b = c, x, 0
    elif h < 2:
        r, g, b = x, c, 0
    elif h < 3:
        r, g, b = 0, c, x
    elif h < 4:
        r, g, b = 0, x, c
    elif h < 5:
        r, g, b = x, 0, c
    else:
        r, g, b = c, 0, x

    def to_hex(n):
        return format(round((n + m) * 255), "02x")

    return f"{to_hex(r)}{to_hex(g)}{to_hex(b)}"


def hex_to_rgb(hex_color: str) -> tuple:
    """Convert hex color to RGB tuple (0-1 range)"""
    hex_color = hex_color.lstrip("#")
    return tuple(int(hex_color[i : i + 2], 16) / 255.0 for i in (0, 2, 4))


def generate_path(from_pos: tuple, to_pos: tuple, target_steps: int = 20) -> list:
    """Generate a human-like mouse path with gravity and wind forces."""
    s3 = math.sqrt(3)
    s5 = math.sqrt(5)

    gravity = 9.0
    wind = 3.0
    max_v = 15.0
    damp_dist = 12.0

    x, y = float(from_pos[0]), float(from_pos[1])
    dx, dy = float(to_pos[0]), float(to_pos[1])

    vx = vy = 0.0
    wx = wy = 0.0
    mv = max_v

    path = [(x, y)]

    for _ in range(500):
        dist = math.hypot(dx - x, dy - y)
        if dist < 1:
            break

        wm = min(wind, dist)

        if dist >= damp_dist:
            wx = wx / s3 + (2 * random.random() - 1) * wm / s5
            wy = wy / s3 + (2 * random.random() - 1) * wm / s5
        else:
            wx /= s3
            wy /= s3
            if mv < 3:
                mv = random.random() * 3 + 3
            else:
                mv /= s5

        vx += wx + gravity * (dx - x) / dist
        vy += wy + gravity * (dy - y) / dist

        vm = math.hypot(vx, vy)
        if vm > mv:
            vc = mv / 2 + random.random() * mv / 2
            vx = (vx / vm) * vc
            vy = (vy / vm) * vc

        x += vx
        y += vy
        path.append((x, y))

    path.append((dx, dy))

    if len(path) > target_steps:
        idx = [int(i * (len(path) - 1) / target_steps) for i in range(target_steps + 1)]
        path = [path[i] for i in idx]

    return path


class OverlayCursor(Gtk.Window):
    def __init__(self, name: str, color: str):
        super().__init__(type=Gtk.WindowType.POPUP)

        self.name = name
        self.color = hex_to_rgb(color)
        self.opacity = 0.0
        self.visible = False
        self.clicking = False
        self.click_anim_progress = 0.0

        self.current_x = 0.0
        self.current_y = 0.0
        self.target_path = []
        self.path_index = 0
        self.move_start_time = 0

        self.last_activity = time.time()
        self.fading = False
        self.masked = False  # True when cursor should be hidden (inside a mask region)
        self.frame_counter = 0  # For debug display

        # Load cursor image
        self.cursor_pixbuf = None
        script_dir = os.path.dirname(os.path.abspath(__file__))
        cursor_path = os.path.join(script_dir, "cursor.png")
        if os.path.exists(cursor_path):
            try:
                self.cursor_pixbuf = GdkPixbuf.Pixbuf.new_from_file_at_scale(
                    cursor_path, CURSOR_SIZE, CURSOR_SIZE, True
                )
                print(f"Loaded cursor image from {cursor_path}", file=sys.stderr)
            except Exception as e:
                print(f"Failed to load cursor image: {e}", file=sys.stderr)
        else:
            print(f"Cursor image not found at {cursor_path}", file=sys.stderr)

        # Window setup (wider/taller to fit cursor and debug label)
        self.set_size_request(CURSOR_SIZE * 5, CURSOR_SIZE * 3)
        self.set_decorated(False)
        self.set_skip_taskbar_hint(True)
        self.set_skip_pager_hint(True)
        self.set_accept_focus(False)
        self.set_app_paintable(True)
        self.set_title(f"cuabot-cursor-{name}")

        # Enable transparency
        screen = self.get_screen()
        visual = screen.get_rgba_visual()
        if visual:
            self.set_visual(visual)

        # Make click-through after window is realized
        self.connect("realize", self._on_realize)

        # Drawing area
        self.drawing_area = Gtk.DrawingArea()
        self.drawing_area.connect("draw", self.on_draw)
        self.add(self.drawing_area)

        # Start animation loop
        GLib.timeout_add(16, self.animation_tick)  # ~60fps

        # Start fade check
        GLib.timeout_add(1000, self.check_fade)

    def _on_realize(self, widget):
        # Make window click-through by setting empty input region
        gdk_window = self.get_window()
        if gdk_window:
            gdk_window.input_shape_combine_region(cairo.Region(), 0, 0)

    def on_draw(self, widget, cr):
        # Clear background
        cr.set_operator(cairo.OPERATOR_SOURCE)
        cr.set_source_rgba(0, 0, 0, 0)
        cr.paint()

        # Debug label - always show when enabled so we can see updates
        if SHOW_MASKED_DEBUG and self.visible:
            cr.set_operator(cairo.OPERATOR_OVER)
            cr.select_font_face("Sans", cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_BOLD)
            cr.set_font_size(14)

            debug_text = f"masked:{int(self.masked)} frame:{self.frame_counter % 10}"
            extents = cr.text_extents(debug_text)

            label_x = CURSOR_SIZE * 0.3
            label_y = CURSOR_SIZE * 2.5
            padding = 6

            # Background
            cr.set_source_rgba(0, 0, 0, 0.9)
            cr.rectangle(
                label_x - padding,
                label_y - extents.height - padding,
                extents.width + padding * 2,
                extents.height + padding * 2,
            )
            cr.fill()

            # Text color: red when masked, green when not
            if self.masked:
                cr.set_source_rgba(1, 0.3, 0.3, 1)  # Red
            else:
                cr.set_source_rgba(0.3, 1, 0.3, 1)  # Green
            cr.move_to(label_x, label_y)
            cr.show_text(debug_text)

        if self.opacity <= 0:
            return

        # Skip rendering cursor if masked (calculation done in cuabotd.ts)
        if self.masked:
            return

        r, g, b = self.color

        # Draw cursor image or fallback circle
        if self.cursor_pixbuf:
            # Create a colorized version of the cursor
            colorized = self._colorize_pixbuf(self.cursor_pixbuf, r, g, b, self.opacity)

            # Click scale effect
            click_scale = (
                1.0 + 0.2 * math.sin(self.click_anim_progress * math.pi) if self.clicking else 1.0
            )

            cr.set_operator(cairo.OPERATOR_OVER)

            if click_scale != 1.0:
                # Scale around the cursor tip (top-left corner)
                scaled_size = int(CURSOR_SIZE * click_scale)
                scaled = colorized.scale_simple(
                    scaled_size, scaled_size, GdkPixbuf.InterpType.BILINEAR
                )
                Gdk.cairo_set_source_pixbuf(
                    cr, scaled, CURSOR_SIZE - CURSOR_SIZE * 0.1, CURSOR_SIZE - CURSOR_SIZE * 0.1
                )
            else:
                Gdk.cairo_set_source_pixbuf(
                    cr, colorized, CURSOR_SIZE - CURSOR_SIZE * 0.1, CURSOR_SIZE - CURSOR_SIZE * 0.1
                )

            cr.paint()

            # Click ripple
            if self.clicking and self.click_anim_progress > 0:
                cx = CURSOR_SIZE + CURSOR_SIZE * 0.1
                cy = CURSOR_SIZE + CURSOR_SIZE * 0.1
                ripple_radius = CURSOR_SIZE * 0.3 + (CURSOR_SIZE * 0.8 * self.click_anim_progress)
                ripple_alpha = (1 - self.click_anim_progress) * 0.5 * self.opacity
                cr.set_source_rgba(r, g, b, ripple_alpha)
                cr.set_line_width(2)
                cr.arc(cx, cy, ripple_radius, 0, 2 * math.pi)
                cr.stroke()
        else:
            # Fallback: draw circle cursor
            cx = CURSOR_SIZE
            cy = CURSOR_SIZE

            # Outer glow
            cr.set_operator(cairo.OPERATOR_OVER)
            gradient = cairo.RadialGradient(cx, cy, CURSOR_SIZE * 0.3, cx, cy, CURSOR_SIZE)
            gradient.add_color_stop_rgba(0, r, g, b, 0.4 * self.opacity)
            gradient.add_color_stop_rgba(1, r, g, b, 0)
            cr.set_source(gradient)
            cr.arc(cx, cy, CURSOR_SIZE, 0, 2 * math.pi)
            cr.fill()

            # Main circle
            base_radius = CURSOR_SIZE * 0.35
            click_scale = (
                1.0 + 0.3 * math.sin(self.click_anim_progress * math.pi) if self.clicking else 1.0
            )
            radius = base_radius * click_scale

            cr.set_source_rgba(r, g, b, 0.9 * self.opacity)
            cr.arc(cx, cy, radius, 0, 2 * math.pi)
            cr.fill()

            # Inner highlight
            cr.set_source_rgba(1, 1, 1, 0.3 * self.opacity)
            cr.arc(cx - radius * 0.2, cy - radius * 0.2, radius * 0.4, 0, 2 * math.pi)
            cr.fill()

            # Click ripple
            if self.clicking and self.click_anim_progress > 0:
                ripple_radius = base_radius + (CURSOR_SIZE * 0.8 * self.click_anim_progress)
                ripple_alpha = (1 - self.click_anim_progress) * 0.5 * self.opacity
                cr.set_source_rgba(r, g, b, ripple_alpha)
                cr.set_line_width(2)
                cr.arc(cx, cy, ripple_radius, 0, 2 * math.pi)
                cr.stroke()

        # # Name label (small, below cursor)
        # cx = CURSOR_SIZE
        # cy = CURSOR_SIZE
        # if self.name and self.opacity > 0.5:
        #     cr.select_font_face("Sans", cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_BOLD)
        #     cr.set_font_size(10)
        #     extents = cr.text_extents(self.name)

        #     padding = 3
        #     color_bar_w = 4
        #     label_x = cx + CURSOR_SIZE * 0.3
        #     label_y = cy + CURSOR_SIZE * 0.5
        #     label_w = color_bar_w + extents.width + padding * 3
        #     label_h = extents.height + padding * 2

        #     # Black background
        #     cr.set_source_rgba(0, 0, 0, 0.7 * self.opacity)
        #     cr.rectangle(label_x, label_y, label_w, label_h)
        #     cr.fill()

        #     # Colored accent bar
        #     cr.set_source_rgba(r, g, b, self.opacity)
        #     cr.rectangle(label_x, label_y, color_bar_w, label_h)
        #     cr.fill()

        #     # Text
        #     cr.set_source_rgba(1, 1, 1, self.opacity)
        #     cr.move_to(label_x + color_bar_w + padding, label_y + extents.height + padding - 1)
        #     cr.show_text(self.name)

    def _colorize_pixbuf(self, pixbuf, r, g, b, opacity):
        """Colorize a greyscale RGBA pixbuf by multiplying with the given color"""
        # Create a copy to modify
        colorized = pixbuf.copy()

        width = colorized.get_width()
        height = colorized.get_height()
        rowstride = colorized.get_rowstride()
        n_channels = colorized.get_n_channels()
        pixels = colorized.get_pixels()

        # Create a new pixel array
        new_pixels = bytearray(pixels)

        for y in range(height):
            for x in range(width):
                idx = y * rowstride + x * n_channels

                # Get greyscale value (use as intensity)
                grey = new_pixels[idx] / 255.0

                # Multiply by color
                new_pixels[idx] = int(grey * r * 255)  # R
                new_pixels[idx + 1] = int(grey * g * 255)  # G
                new_pixels[idx + 2] = int(grey * b * 255)  # B

                # Apply opacity to alpha channel
                if n_channels == 4:
                    new_pixels[idx + 3] = int(new_pixels[idx + 3] * opacity)

        # Create new pixbuf from modified pixels
        return GdkPixbuf.Pixbuf.new_from_data(
            bytes(new_pixels),
            GdkPixbuf.Colorspace.RGB,
            n_channels == 4,
            8,
            width,
            height,
            rowstride,
        )

    def animation_tick(self):
        needs_redraw = False
        self.frame_counter += 1

        # Always redraw when debugging to show frame updates
        if SHOW_MASKED_DEBUG:
            needs_redraw = True

        # Handle movement animation
        if self.target_path and self.path_index < len(self.target_path):
            elapsed = time.time() - self.move_start_time
            progress = min(1.0, elapsed / MOVE_DURATION)

            # Map progress to path index
            target_index = int(progress * (len(self.target_path) - 1))
            if target_index > self.path_index:
                self.path_index = target_index
                point = self.target_path[self.path_index]
                self.current_x = point[0]
                self.current_y = point[1]
                self.move(int(self.current_x - CURSOR_SIZE), int(self.current_y - CURSOR_SIZE))
                needs_redraw = True

            if progress >= 1.0:
                # Movement complete
                self.target_path = []
                self.path_index = 0

        # Handle click animation
        if self.clicking:
            self.click_anim_progress += 0.1
            if self.click_anim_progress >= 1.0:
                self.clicking = False
                self.click_anim_progress = 0.0
            needs_redraw = True

        # Handle fade in
        if self.visible and self.opacity < 1.0 and not self.fading:
            self.opacity = min(1.0, self.opacity + 0.1)
            needs_redraw = True

        # Handle fade out
        if self.fading:
            self.opacity = max(0.0, self.opacity - 0.05)
            needs_redraw = True
            if self.opacity <= 0:
                # Reset state so cursor can reappear on next move
                self.fading = False
                self.visible = False
                self.hide()

        if needs_redraw:
            self.drawing_area.queue_draw()

        return True  # Continue timer

    def check_fade(self):
        if not self.fading and self.visible:
            idle_time = time.time() - self.last_activity
            if idle_time >= FADE_DELAY:
                self.fading = True
        return True

    def move_to(self, x: int, y: int, click: bool = False):
        """Move cursor to position with smooth animation"""
        self.last_activity = time.time()
        self.fading = False

        if click:
            self.clicking = True
            self.click_anim_progress = 0.0

        target = (float(x), float(y))

        if not self.visible:
            # First move - spawn from random direction
            self.visible = True
            self.show_all()

            angle = random.random() * 2 * math.pi
            spawn_x = x + math.cos(angle) * SPAWN_DISTANCE
            spawn_y = y + math.sin(angle) * SPAWN_DISTANCE

            self.current_x = spawn_x
            self.current_y = spawn_y
            self.move(int(spawn_x - CURSOR_SIZE), int(spawn_y - CURSOR_SIZE))

        # Generate smooth path
        self.target_path = generate_path((self.current_x, self.current_y), target)
        self.path_index = 0
        self.move_start_time = time.time()

    def play_click(self):
        """Play click animation at current position"""
        self.last_activity = time.time()
        self.fading = False
        self.clicking = True
        self.click_anim_progress = 0.0

    def hide_cursor(self):
        """Immediately hide the cursor"""
        self.opacity = 0.0
        self.visible = False
        self.fading = False
        self.hide()

    def set_masked(self, masked: bool):
        """Set whether cursor is masked (should be hidden)."""
        changed = self.masked != masked
        self.masked = masked
        # Always queue redraw to update debug label and cursor visibility
        # Don't call hide()/show_all() - let on_draw handle the rendering
        self.drawing_area.queue_draw()
        if changed:
            print(f"[overlay] masked={masked}", file=sys.stderr)


class SocketServer(threading.Thread):
    def __init__(self, cursor: OverlayCursor):
        super().__init__(daemon=True)
        self.cursor = cursor
        self.running = True

    def run(self):
        # Remove existing socket
        try:
            os.unlink(SOCKET_PATH)
        except OSError:
            pass

        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(SOCKET_PATH)
        server.listen(5)
        os.chmod(SOCKET_PATH, 0o777)

        while self.running:
            try:
                server.settimeout(1.0)
                conn, _ = server.accept()
                data = conn.recv(4096).decode("utf-8")
                conn.close()

                for line in data.strip().split("\n"):
                    if not line:
                        continue
                    try:
                        cmd = json.loads(line)
                        self.handle_command(cmd)
                    except json.JSONDecodeError:
                        pass
            except socket.timeout:
                continue
            except Exception as e:
                print(f"Socket error: {e}", file=sys.stderr)

    def handle_command(self, cmd: dict):
        cmd_type = cmd.get("type")

        if cmd_type == "move":
            x = cmd.get("x", 0)
            y = cmd.get("y", 0)
            click = cmd.get("click", False)
            GLib.idle_add(self.cursor.move_to, x, y, click)

        elif cmd_type == "click":
            GLib.idle_add(self.cursor.play_click)

        elif cmd_type == "hide":
            GLib.idle_add(self.cursor.hide_cursor)

        elif cmd_type == "masked":
            value = cmd.get("value", False)
            GLib.idle_add(self.cursor.set_masked, value)

        elif cmd_type == "quit":
            GLib.idle_add(Gtk.main_quit)


def send_command(cmd: dict) -> bool:
    """Send command to running overlay cursor"""
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(SOCKET_PATH)
        sock.send((json.dumps(cmd) + "\n").encode("utf-8"))
        sock.close()
        return True
    except:
        return False


def main():
    # Parse arguments
    name = os.environ.get("CUABOT_NAME", "cuabot")
    color = os.environ.get("CUABOT_COLOR", "")

    for arg in sys.argv[1:]:
        if arg.startswith("--name="):
            name = arg.split("=", 1)[1]
        elif arg.startswith("--color="):
            color = arg.split("=", 1)[1]

    # Generate color from name if not provided
    if not color:
        color = name_to_color(name)

    color = color.lstrip("#")

    # Check if already running - send move command instead
    if os.path.exists(SOCKET_PATH):
        # Try to connect
        if send_command({"type": "ping"}):
            print("Overlay cursor already running", file=sys.stderr)
            sys.exit(0)
        else:
            # Stale socket, remove it
            try:
                os.unlink(SOCKET_PATH)
            except:
                pass

    print(f"Starting overlay cursor: name={name}, color=#{color}", file=sys.stderr)

    # Initialize GTK
    Gtk.init(sys.argv)

    # Create cursor window
    cursor = OverlayCursor(name, color)

    # Start socket server
    server = SocketServer(cursor)
    server.start()

    # Run GTK main loop
    try:
        Gtk.main()
    except KeyboardInterrupt:
        pass
    finally:
        server.running = False
        try:
            os.unlink(SOCKET_PATH)
        except:
            pass


if __name__ == "__main__":
    main()
