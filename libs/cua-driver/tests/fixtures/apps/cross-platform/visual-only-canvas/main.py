#!/usr/bin/env python3
"""Custom-painted visual fixture with a loopback-only behavioral oracle."""

from __future__ import annotations

import argparse
import ast
import json
import os
import re
import subprocess
import sys
import tkinter as tk
import time
from typing import Callable, Optional
import urllib.parse
import urllib.request


WIDTH = 760
HEIGHT = 460
CARD_LABEL_FONT_PIXELS = 32
DEFAULT_TITLE = "Cua Visual-Only Canvas Fixture"
X11_DISCOVERY_ATTEMPTS = 20
X11_DISCOVERY_INTERVAL_SECONDS = 0.05
CARDS = (
    {"id": "save", "label": "Save", "bounds": (72, 132, 276, 310), "color": "#e85d3f"},
    {"id": "send", "label": "Send", "bounds": (292, 132, 496, 310), "color": "#1e8b99"},
    {"id": "cancel", "label": "Cancel", "bounds": (512, 132, 716, 310), "color": "#6f8f3d"},
)


def card_at(x: int, y: int) -> Optional[str]:
    for card in CARDS:
        left, top, right, bottom = card["bounds"]
        if left <= x <= right and top <= y <= bottom:
            return str(card["id"])
    return None


def bind_background_click(
    toplevel: tk.Misc,
    canvas: tk.Canvas,
    handler: Callable[[tk.Event], str],
) -> None:
    # X11 targets the deepest child while Windows targeted injection may reach
    # the toplevel. Returning "break" keeps a canvas event from firing twice as
    # Tk walks from the widget bindtag to the toplevel bindtag.
    canvas.bind("<Button-1>", handler)
    toplevel.bind("<Button-1>", handler)


def canvas_point(event: tk.Event, canvas: tk.Canvas) -> tuple[int, int]:
    return (
        int(event.x_root) - canvas.winfo_rootx(),
        int(event.y_root) - canvas.winfo_rooty(),
    )


def oracle_state(selected: Optional[str], action_count: int) -> dict[str, object]:
    return {
        "fixture": "visual-only-canvas/v1",
        "pid": os.getpid(),
        "ready": True,
        "selected": selected,
        "action_count": action_count,
    }


def validate_journal_url(url: str) -> None:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "::1", "localhost"}:
        raise ValueError("fixture journal must be an HTTP loopback endpoint")


def post_oracle(url: str, state: dict[str, object]) -> None:
    validate_journal_url(url)
    payload = json.dumps(state, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=1) as response:
        if response.status != 204:
            raise RuntimeError(f"fixture journal returned HTTP {response.status}")


def _xprop(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["xprop", *args],
        check=True,
        capture_output=True,
        text=True,
        timeout=1,
    )


def _subprocess_diagnostic(error: OSError | subprocess.SubprocessError) -> str:
    stderr = getattr(error, "stderr", None)
    if isinstance(stderr, bytes):
        stderr = stderr.decode(errors="replace")
    detail = stderr.strip() if isinstance(stderr, str) else ""
    return f"{error}; stderr={detail!r}" if detail else str(error)


def _x11_client_ids(output: str) -> list[str]:
    matches = re.findall(r"\b0x[0-9a-fA-F]+\b", output)
    return list(dict.fromkeys(match.lower() for match in matches if match != "0x0"))


def _x11_window_title(output: str) -> Optional[str]:
    titles: dict[str, str] = {}
    for line in output.splitlines():
        match = re.match(
            r"^(?P<name>_NET_WM_NAME|WM_NAME)(?:\([^)]*\))?\s*=\s*(?P<value>.+)$",
            line,
        )
        if match is None:
            continue
        try:
            value = ast.literal_eval(match.group("value"))
        except (SyntaxError, ValueError):
            continue
        if isinstance(value, str):
            titles[match.group("name")] = value
    return titles.get("_NET_WM_NAME", titles.get("WM_NAME"))


def _x11_property_number(output: str, property_name: str) -> Optional[int]:
    match = re.search(
        rf"^{re.escape(property_name)}(?:\([^)]*\))?\s*=\s*(\d+)\s*$",
        output,
        re.MULTILINE,
    )
    return int(match.group(1)) if match is not None else None


def publish_x11_owner(
    title: str,
    *,
    attempts: int = X11_DISCOVERY_ATTEMPTS,
    interval_seconds: float = X11_DISCOVERY_INTERVAL_SECONDS,
) -> None:
    if not sys.platform.startswith("linux"):
        return

    if attempts < 1:
        raise ValueError("X11 discovery attempts must be positive")

    last_diagnostic = "root client list was not queried"
    window_id: Optional[str] = None
    for attempt in range(1, attempts + 1):
        try:
            clients = _xprop("-root", "_NET_CLIENT_LIST_STACKING", "_NET_CLIENT_LIST")
            client_ids = _x11_client_ids(clients.stdout)
            matches: list[str] = []
            observed: list[str] = []
            for client_id in client_ids:
                try:
                    properties = _xprop("-id", client_id, "_NET_WM_NAME", "WM_NAME")
                except (OSError, subprocess.SubprocessError) as error:
                    observed.append(f"{client_id}=<query failed: {_subprocess_diagnostic(error)}>")
                    continue
                client_title = _x11_window_title(properties.stdout)
                observed.append(f"{client_id}={client_title!r}")
                if client_title == title:
                    matches.append(client_id)
            if len(matches) > 1:
                raise RuntimeError(
                    f"X11 title {title!r} matched multiple EWMH clients: {', '.join(matches)}"
                )
            if matches:
                window_id = matches[0]
                break
            displayed = observed[:12]
            if len(observed) > len(displayed):
                displayed.append(f"... {len(observed) - len(displayed)} more")
            last_diagnostic = (
                f"attempt {attempt}/{attempts}: {len(client_ids)} EWMH clients; "
                f"observed {', '.join(displayed) if displayed else '<none>'}"
            )
        except RuntimeError:
            raise
        except (OSError, subprocess.SubprocessError) as error:
            last_diagnostic = (
                f"attempt {attempt}/{attempts}: xprop failed: {_subprocess_diagnostic(error)}"
            )
        if attempt < attempts:
            time.sleep(interval_seconds)

    if window_id is None:
        raise RuntimeError(
            f"could not find unique mapped X11 client titled {title!r}; {last_diagnostic}"
        )

    expected_pid = os.getpid()
    try:
        _xprop(
            "-id",
            window_id,
            "-f",
            "_NET_WM_PID",
            "32c",
            "-set",
            "_NET_WM_PID",
            str(expected_pid),
        )
        published = _xprop("-id", window_id, "_NET_WM_PID")
    except (OSError, subprocess.SubprocessError) as error:
        raise RuntimeError(
            f"failed to publish _NET_WM_PID on X11 client {window_id}: "
            f"{_subprocess_diagnostic(error)}"
        ) from error
    actual_pid = _x11_property_number(published.stdout, "_NET_WM_PID")
    if actual_pid != expected_pid:
        raise RuntimeError(
            f"X11 client {window_id} reported _NET_WM_PID={actual_pid!r}; expected {expected_pid}"
        )


class VisualFixture:
    def __init__(self, journal_url: str, title: str = DEFAULT_TITLE) -> None:
        self.journal_url = journal_url
        self.selected: Optional[str] = None
        self.action_count = 0
        self.root = tk.Tk()
        self.root.title(title)
        self.root.geometry(f"{WIDTH}x{HEIGHT}")
        self.root.resizable(False, False)
        self.root.update_idletasks()
        self.canvas = tk.Canvas(
            self.root,
            width=WIDTH,
            height=HEIGHT,
            background="#f3ead7",
            highlightthickness=0,
            takefocus=0,
        )
        self.canvas.pack(fill="both", expand=True)
        bind_background_click(self.root, self.canvas, self.on_click)
        self.paint()
        if sys.platform.startswith("linux"):
            self.root.update()
            publish_x11_owner(title)
        self.root.after(50, self.publish)

    def paint(self) -> None:
        self.canvas.delete("all")
        self.canvas.create_rectangle(0, 0, WIDTH, 94, fill="#172b36", outline="")
        self.canvas.create_text(
            42, 34, anchor="nw", text="CHOOSE A SIGNAL", fill="#fff8e8", font=("Helvetica", 25, "bold")
        )
        self.canvas.create_text(
            44, 72, anchor="nw", text="A painted surface: pixels in, coordinates out", fill="#b8c9c8", font=("Helvetica", 11)
        )
        for card in CARDS:
            left, top, right, bottom = card["bounds"]
            selected = self.selected == card["id"]
            border = "#172b36" if selected else "#cfbea0"
            width = 7 if selected else 2
            self.canvas.create_rectangle(left, top, right, bottom, fill=card["color"], outline=border, width=width)
            self.canvas.create_oval(left + 61, top + 42, left + 143, top + 124, fill="#fff8e8", outline="")
            self.canvas.create_text(
                (left + right) // 2,
                bottom - 28,
                text=card["label"],
                fill="#fff8e8",
                font=("Helvetica", -CARD_LABEL_FONT_PIXELS, "bold"),
            )
        status = "WAITING FOR A VISUAL CHOICE" if self.selected is None else f"SELECTED: {self.selected.upper()}"
        self.canvas.create_text(380, 386, text=status, fill="#172b36", font=("Helvetica", 15, "bold"))

    def on_click(self, event: tk.Event) -> str:
        x, y = canvas_point(event, self.canvas)
        selected = card_at(x, y)
        if selected is not None:
            self.selected = selected
            self.action_count += 1
            self.paint()
            self.publish()
        else:
            print(
                f"visual-only-canvas ignored click outside cards at canvas ({x}, {y})",
                file=sys.stderr,
                flush=True,
            )
        return "break"

    def publish(self) -> None:
        try:
            post_oracle(self.journal_url, oracle_state(self.selected, self.action_count))
        except OSError:
            self.root.after(100, self.publish)

    def run(self) -> None:
        self.root.mainloop()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--journal-url")
    parser.add_argument("--title", default=DEFAULT_TITLE)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        assert args.title
        assert CARD_LABEL_FONT_PIXELS == 32
        for card in CARDS:
            left, _, right, _ = card["bounds"]
            label = str(card["label"])
            assert label
            assert len(label) * CARD_LABEL_FONT_PIXELS <= right - left
        assert card_at(174, 220) == "save"
        assert card_at(394, 220) == "send"
        assert card_at(614, 220) == "cancel"
        assert card_at(20, 20) is None
        assert oracle_state("send", 2)["action_count"] == 2
        return 0
    if not args.journal_url:
        parser.error("--journal-url is required unless --self-test is used")
    validate_journal_url(args.journal_url)
    VisualFixture(args.journal_url, args.title).run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
